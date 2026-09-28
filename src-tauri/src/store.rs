use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tokio::sync::{Notify, RwLock};
use tracing::{debug, error, info, warn};

use crate::error::{AppError, AppResult};
use crate::github::GithubPrStatus;
use crate::state::{AppState, AgentSessionMeta, Folder, Workspace, WorkspaceStatus};

/// Lets `Store` persist-and-notify without depending on Tauri.
pub trait WorkspaceNotifier: Send + Sync + 'static {
    fn workspace_changed(&self, workspace_id: &str);
}

#[cfg(test)]
pub struct NullNotifier;

#[cfg(test)]
impl WorkspaceNotifier for NullNotifier {
    fn workspace_changed(&self, _workspace_id: &str) {}
}

pub struct Store {
    state: Arc<RwLock<AppState>>,
    dirty: Arc<Notify>,
    state_path: PathBuf,
    tmp_path: PathBuf,
    notifier: Box<dyn WorkspaceNotifier>,
}

const DEBOUNCE: Duration = Duration::from_millis(250);

impl Store {
    pub async fn load(
        state_path: PathBuf,
        tmp_path: PathBuf,
        notifier: Box<dyn WorkspaceNotifier>,
    ) -> AppResult<Arc<Self>> {
        let raw = match tokio::fs::read(&state_path).await {
            Ok(bytes) if !bytes.is_empty() => Some(bytes),
            Ok(_) => None,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                info!("no state.json yet; starting empty");
                None
            }
            Err(e) => return Err(e.into()),
        };

        let mut initial = match raw.as_deref() {
            Some(bytes) => match serde_json::from_slice::<AppState>(bytes) {
                Ok(s) => {
                    info!(workspaces = s.workspaces.len(), "loaded state.json");
                    s
                }
                Err(e) => {
                    error!(error = %e, "state.json failed to parse; starting empty");
                    AppState::default()
                }
            },
            None => AppState::default(),
        };

        if let Some(bytes) = raw.as_deref() {
            migrate_archived_to_folder(&mut initial, bytes);
            migrate_pr_slots(&mut initial, bytes);
            migrate_sessions(&mut initial, bytes);
        }

        let stranded = initial.prune_missing_folders();
        if stranded > 0 {
            warn!(
                count = stranded,
                "workspaces named a folder that doesn't exist; moved to Default"
            );
        }

        // Drafts from a previous run have lost their progress stream; the
        // reconciler handles anything they left on disk.
        let pruned = initial
            .workspaces
            .iter()
            .filter(|w| !matches!(w.status, WorkspaceStatus::Ready))
            .map(|w| w.id.clone())
            .collect::<Vec<_>>();
        if !pruned.is_empty() {
            info!(count = pruned.len(), "pruning non-Ready workspaces from state");
            initial
                .workspaces
                .retain(|w| matches!(w.status, WorkspaceStatus::Ready));
            // `blocks_caller` makes drafts legal blockers.
            for id in &pruned {
                initial.clear_links_to(id);
            }
        }

        let missing_pages: usize = initial
            .workspaces
            .iter_mut()
            .map(|w| crate::artifacts::prune_missing_pages(&mut w.artifacts))
            .sum();
        if missing_pages > 0 {
            info!(count = missing_pages, "dropped page artifacts whose files are gone");
        }

        let store = Arc::new(Self {
            state: Arc::new(RwLock::new(initial)),
            dirty: Arc::new(Notify::new()),
            state_path,
            tmp_path,
            notifier,
        });

        store.clone().spawn_flusher();
        Ok(store)
    }

    pub async fn read<R, F: FnOnce(&AppState) -> R>(&self, f: F) -> R {
        let guard = self.state.read().await;
        f(&guard)
    }

    pub async fn mutate<R, F>(&self, f: F) -> AppResult<R>
    where
        F: FnOnce(&mut AppState) -> AppResult<R>,
    {
        let result = {
            let mut guard = self.state.write().await;
            f(&mut guard)?
        };
        self.dirty.notify_one();
        Ok(result)
    }

    pub async fn with_workspace<R, F>(&self, id: &str, f: F) -> AppResult<R>
    where
        F: FnOnce(&Workspace) -> R,
    {
        let guard = self.state.read().await;
        let ws = guard
            .find_workspace(id)
            .ok_or_else(|| AppError::WorkspaceNotFound(id.to_string()))?;
        Ok(f(ws))
    }

    pub async fn update_workspace<R, F>(&self, id: &str, f: F) -> AppResult<R>
    where
        F: FnOnce(&mut Workspace) -> AppResult<R>,
    {
        let result = self.update_workspace_quiet(id, f).await?;
        self.notifier.workspace_changed(id);
        Ok(result)
    }

    pub fn notify_workspace_changed(&self, id: &str) {
        self.notifier.workspace_changed(id);
    }

    /// For edits the UI already shows locally, e.g. notes typing, where an
    /// echo would fight the cursor.
    pub async fn update_workspace_quiet<R, F>(&self, id: &str, f: F) -> AppResult<R>
    where
        F: FnOnce(&mut Workspace) -> AppResult<R>,
    {
        let result = {
            let mut guard = self.state.write().await;
            let ws = guard
                .find_workspace_mut(id)
                .ok_or_else(|| AppError::WorkspaceNotFound(id.to_string()))?;
            f(ws)?
        };
        self.dirty.notify_one();
        Ok(result)
    }

    pub fn notify_changed(&self, workspace_id: &str) {
        self.notifier.workspace_changed(workspace_id);
    }

    fn spawn_flusher(self: Arc<Self>) {
        tokio::spawn(async move {
            loop {
                self.dirty.notified().await;
                tokio::time::sleep(DEBOUNCE).await;

                if let Err(e) = self.flush().await {
                    error!(error = %e, "flush failed");
                }
            }
        });
    }

    async fn flush(&self) -> AppResult<()> {
        let snapshot = {
            let guard = self.state.read().await;
            serde_json::to_vec_pretty(&*guard)?
        };

        if let Some(parent) = self.state_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        tokio::fs::write(&self.tmp_path, &snapshot).await?;

        match tokio::fs::File::options()
            .write(true)
            .open(&self.tmp_path)
            .await
        {
            Ok(f) => {
                if let Err(e) = f.sync_all().await {
                    warn!(error = %e, "fsync of state.json.tmp failed");
                }
            }
            Err(e) => warn!(error = %e, "reopen of state.json.tmp for fsync failed"),
        }

        tokio::fs::rename(&self.tmp_path, &self.state_path).await?;
        debug!(bytes = snapshot.len(), "flushed state.json");
        Ok(())
    }
}

// The migrations read retired fields off raw JSON so the types carry no trace
// of them; the first flush drops the fields and ends the migration.
fn migrate_archived_to_folder(state: &mut AppState, raw: &[u8]) {
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(raw) else {
        return;
    };
    let archived: Vec<String> = json
        .get("workspaces")
        .and_then(|w| w.as_array())
        .map(|list| {
            list.iter()
                .filter(|w| w.get("archived_at").is_some_and(|a| !a.is_null()))
                .filter_map(|w| w.get("id").and_then(|i| i.as_str()))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if archived.is_empty() {
        return;
    }

    let mut folder = Folder::new("Archived");
    folder.collapsed = true;
    let folder_id = folder.id.clone();
    let mut moved = 0;
    for ws in &mut state.workspaces {
        if archived.contains(&ws.id) {
            ws.folder = Some(folder_id.clone());
            moved += 1;
        }
    }
    if moved == 0 {
        return;
    }
    state.folders.push(folder);
    info!(count = moved, "migrated archived workspaces into a folder");
}

/// `track`, not push: a PR could sit in both `github` and `attached_prs`.
fn migrate_pr_slots(state: &mut AppState, raw: &[u8]) {
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(raw) else {
        return;
    };
    let Some(workspaces) = json.get("workspaces").and_then(|w| w.as_array()) else {
        return;
    };

    let mut migrated = 0usize;
    for ws_json in workspaces {
        let Some(id) = ws_json.get("id").and_then(|i| i.as_str()) else {
            continue;
        };
        let Some(links) = ws_json.get("repo_links").and_then(|l| l.as_array()) else {
            continue;
        };
        for link_json in links {
            let Some(repo_key) = link_json.get("repo_key").and_then(|k| k.as_str()) else {
                continue;
            };
            let branch_pr: Option<GithubPrStatus> = link_json
                .get("github")
                .filter(|g| !g.is_null())
                .and_then(|g| serde_json::from_value(g.clone()).ok());
            let attached: Vec<OldAttachedPr> = link_json
                .get("attached_prs")
                .and_then(|a| serde_json::from_value(a.clone()).ok())
                .unwrap_or_default();
            if branch_pr.is_none() && attached.is_empty() {
                continue;
            }

            let Some(link) = state
                .workspaces
                .iter_mut()
                .find(|w| w.id == id)
                .and_then(|w| w.link_mut(repo_key))
            else {
                continue;
            };
            if !link.prs.is_empty() {
                continue;
            }
            if let Some(status) = branch_pr {
                link.track(status.pr_number, Some(status));
                migrated += 1;
            }
            for old in attached {
                link.track(old.number, old.status);
                migrated += 1;
            }
        }
    }
    if migrated > 0 {
        info!(count = migrated, "migrated PRs into the tracked-PR list");
    }
}

#[derive(Deserialize)]
struct OldAttachedPr {
    number: u32,
    #[serde(default)]
    status: Option<GithubPrStatus>,
}

fn migrate_sessions(state: &mut AppState, raw: &[u8]) {
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(raw) else {
        return;
    };
    let Some(workspaces) = json.get("workspaces").and_then(|w| w.as_array()) else {
        return;
    };

    let mut migrated = 0usize;
    let mut dropped = 0usize;
    for ws_json in workspaces {
        let Some(id) = ws_json.get("id").and_then(|i| i.as_str()) else {
            continue;
        };
        let Some(list) = ws_json.get("sessions").and_then(|s| s.as_array()) else {
            continue;
        };
        let Some(chosen) = pick_surviving_session(list) else {
            continue;
        };
        let Some(ws) = state.workspaces.iter_mut().find(|w| w.id == id) else {
            continue;
        };
        if ws.session.is_some() {
            continue;
        }
        match serde_json::from_value::<AgentSessionMeta>(chosen.clone()) {
            Ok(meta) => {
                ws.session = Some(meta);
                if ws.agent_binary.is_none() {
                    ws.agent_binary = chosen
                        .get("claude_binary")
                        .and_then(|b| b.as_str())
                        .map(str::to_string);
                }
                migrated += 1;
                dropped += list.len() - 1;
            }
            Err(e) => warn!(workspace = id, error = %e, "old session entry failed to parse"),
        }
    }
    if migrated > 0 {
        info!(count = migrated, dropped, "migrated session lists into one session each");
    }
}

/// The list was append-ordered, so newest is last.
fn pick_surviving_session(list: &[serde_json::Value]) -> Option<&serde_json::Value> {
    let visible = |s: &&serde_json::Value| {
        !s.get("hidden").and_then(|h| h.as_bool()).unwrap_or(false)
    };
    let has_conversation = |s: &&serde_json::Value| {
        s.get("claude_session_id").is_some_and(|c| !c.is_null())
    };
    list.iter()
        .rev()
        .filter(visible)
        .find(has_conversation)
        .or_else(|| list.iter().rev().find(visible))
        .or_else(|| list.last())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Origin, Workspace, WorkspaceStatus};
    use chrono::Utc;
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingNotifier(Arc<Mutex<Vec<String>>>);

    impl WorkspaceNotifier for RecordingNotifier {
        fn workspace_changed(&self, workspace_id: &str) {
            self.0.lock().unwrap().push(workspace_id.to_string());
        }
    }

    fn workspace(id: &str, status: WorkspaceStatus) -> Workspace {
        Workspace {
            id: id.into(),
            branch: format!("feat/{id}"),
            created_at: Utc::now(),
            repo_links: Vec::new(),
            session: None,
            agent: Default::default(),
            agent_binary: None,
            origin: Origin::Ui,
            deleted_at: None,
            folder: None,
            status,
            notes: String::new(),
            blocked_by: None,
            artifacts: Vec::new(),
        }
    }

    struct Fixture {
        store: Arc<Store>,
        notified: Arc<Mutex<Vec<String>>>,
        _tmp: tempfile::TempDir,
    }

    async fn fixture_from(raw: Option<String>) -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let state_path = tmp.path().join("state.json");
        if let Some(raw) = raw {
            std::fs::write(&state_path, raw).unwrap();
        }
        let notified = Arc::new(Mutex::new(Vec::new()));
        let store = Store::load(
            state_path,
            tmp.path().join("state.json.tmp"),
            Box::new(RecordingNotifier(notified.clone())),
        )
        .await
        .unwrap();
        Fixture {
            store,
            notified,
            _tmp: tmp,
        }
    }

    async fn fixture() -> Fixture {
        fixture_from(None).await
    }

    async fn fixture_with(state: Option<AppState>) -> Fixture {
        fixture_from(state.map(|s| serde_json::to_string(&s).unwrap())).await
    }

    async fn fixture_with_raw(raw: &str) -> Fixture {
        fixture_from(Some(raw.to_string())).await
    }

    fn archived_state_json() -> String {
        r#"{
            "workspaces": [
                {
                    "id": "kept",
                    "branch": "feat/kept",
                    "created_at": "2026-04-01T12:00:00Z",
                    "repo_links": [],
                    "status": {"kind": "ready"}
                },
                {
                    "id": "shelved",
                    "branch": "feat/shelved",
                    "created_at": "2026-04-01T12:00:00Z",
                    "repo_links": [],
                    "status": {"kind": "ready"},
                    "archived_at": "2026-05-01T09:00:00Z"
                },
                {
                    "id": "also-shelved",
                    "branch": "feat/also",
                    "created_at": "2026-04-01T12:00:00Z",
                    "repo_links": [],
                    "status": {"kind": "ready"},
                    "archived_at": "2026-05-02T09:00:00Z"
                }
            ]
        }"#
        .to_string()
    }

    #[tokio::test]
    async fn starts_empty_when_there_is_no_state_file() {
        let f = fixture().await;
        assert_eq!(f.store.read(|s| s.workspaces.len()).await, 0);
    }

    #[tokio::test]
    async fn boot_prunes_workspaces_that_never_reached_ready() {
        let state = AppState {
            workspaces: vec![
                workspace("ready", WorkspaceStatus::Ready),
                workspace("mid-provision", WorkspaceStatus::Creating),
                workspace(
                    "failed",
                    WorkspaceStatus::CreationFailed {
                        error: "boom".into(),
                    },
                ),
            ],
            ..Default::default()
        };
        let f = fixture_with(Some(state)).await;

        let ids = f
            .store
            .read(|s| s.workspaces.iter().map(|w| w.id.clone()).collect::<Vec<_>>())
            .await;
        assert_eq!(ids, vec!["ready"]);
    }

    #[tokio::test]
    async fn update_workspace_mutates_and_notifies() {
        let state = AppState {
            workspaces: vec![workspace("ws-1", WorkspaceStatus::Ready)],
            ..Default::default()
        };
        let f = fixture_with(Some(state)).await;

        f.store
            .update_workspace("ws-1", |ws| {
                ws.notes = "hello".into();
                Ok(())
            })
            .await
            .unwrap();

        assert_eq!(
            f.store.with_workspace("ws-1", |w| w.notes.clone()).await.unwrap(),
            "hello"
        );
        assert_eq!(*f.notified.lock().unwrap(), vec!["ws-1"]);
    }

    #[tokio::test]
    async fn update_workspace_reports_a_missing_workspace() {
        let f = fixture().await;
        let err = f
            .store
            .update_workspace("nope", |_| Ok(()))
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::WorkspaceNotFound(id) if id == "nope"));
        assert!(f.notified.lock().unwrap().is_empty(), "no notify on failure");
    }

    #[tokio::test]
    async fn a_failing_closure_does_not_notify() {
        let state = AppState {
            workspaces: vec![workspace("ws-1", WorkspaceStatus::Ready)],
            ..Default::default()
        };
        let f = fixture_with(Some(state)).await;

        let err = f
            .store
            .update_workspace("ws-1", |_| -> AppResult<()> {
                Err(AppError::Other("nope".into()))
            })
            .await
            .unwrap_err();

        assert!(err.to_string().contains("nope"));
        assert!(f.notified.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_quiet_variant_persists_without_notifying() {
        let state = AppState {
            workspaces: vec![workspace("ws-1", WorkspaceStatus::Ready)],
            ..Default::default()
        };
        let f = fixture_with(Some(state)).await;

        f.store
            .update_workspace_quiet("ws-1", |ws| {
                ws.notes = "typing…".into();
                Ok(())
            })
            .await
            .unwrap();

        assert_eq!(
            f.store.with_workspace("ws-1", |w| w.notes.clone()).await.unwrap(),
            "typing…"
        );
        assert!(f.notified.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn with_workspace_reports_a_missing_workspace() {
        let f = fixture().await;
        let err = f.store.with_workspace("nope", |w| w.id.clone()).await.unwrap_err();
        assert!(matches!(err, AppError::WorkspaceNotFound(id) if id == "nope"));
    }

    #[tokio::test]
    async fn writes_land_at_the_real_path_after_the_debounce() {
        let tmp = tempfile::tempdir().unwrap();
        let state_path = tmp.path().join("state.json");
        let store = Store::load(
            state_path.clone(),
            tmp.path().join("state.json.tmp"),
            Box::new(NullNotifier),
        )
        .await
        .unwrap();

        store
            .mutate(|s| {
                s.workspaces.push(workspace("ws-1", WorkspaceStatus::Ready));
                Ok(())
            })
            .await
            .unwrap();

        tokio::time::sleep(DEBOUNCE * 3).await;

        let written: AppState =
            serde_json::from_slice(&std::fs::read(&state_path).unwrap()).unwrap();
        assert_eq!(written.workspaces.len(), 1);
        assert_eq!(written.workspaces[0].id, "ws-1");
        assert!(
            !tmp.path().join("state.json.tmp").exists(),
            "temp file is renamed away, not left behind"
        );
    }

    #[tokio::test]
    async fn unparseable_state_json_starts_empty_instead_of_failing() {
        let f = fixture_with_raw("{ this is not json").await;
        assert_eq!(f.store.read(|s| s.workspaces.len()).await, 0);
    }

    #[tokio::test]
    async fn boot_migrates_archived_workspaces_into_a_folder() {
        let f = fixture_with_raw(&archived_state_json()).await;

        let (folders, membership) = f
            .store
            .read(|s| {
                (
                    s.folders.clone(),
                    s.workspaces
                        .iter()
                        .map(|w| (w.id.clone(), w.folder.clone()))
                        .collect::<Vec<_>>(),
                )
            })
            .await;

        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].name, "Archived");
        assert!(folders[0].collapsed, "migrated folder starts collapsed");

        let archived = Some(folders[0].id.clone());
        assert_eq!(
            membership,
            vec![
                ("kept".to_string(), None),
                ("shelved".to_string(), archived.clone()),
                ("also-shelved".to_string(), archived),
            ]
        );
    }

    #[tokio::test]
    async fn the_migration_does_not_run_twice() {
        let f = fixture_with_raw(&archived_state_json()).await;
        let migrated = f.store.read(|s| s.clone()).await;

        let again = fixture_with(Some(migrated)).await;
        let folders = again.store.read(|s| s.folders.clone()).await;
        assert_eq!(folders.len(), 1, "one Archived folder, not two");
    }

    /// PR 7 sits in both the branch slot and `attached_prs`.
    fn two_slot_state_json() -> String {
        let pr = |number: u32, branch: &str| {
            format!(
                r#"{{
                    "pr_number": {number},
                    "url": "https://github.com/me/api/pull/{number}",
                    "state": "open",
                    "is_draft": false,
                    "checks": "success",
                    "unresolved_threads": 0,
                    "head_branch": "{branch}",
                    "head_sha": "sha{number}",
                    "fetched_at": "2026-05-01T09:00:00Z"
                }}"#
            )
        };
        format!(
            r#"{{
            "workspaces": [
                {{
                    "id": "ws-0",
                    "branch": "feat/thing",
                    "created_at": "2026-04-01T12:00:00Z",
                    "status": {{"kind": "ready"}},
                    "repo_links": [
                        {{
                            "repo_key": "api",
                            "worktree_path": "/tmp/ws-0/api",
                            "setup_script_ran_at": null,
                            "github": {branch_pr},
                            "attached_prs": [
                                {{
                                    "number": 8,
                                    "attached_at": "2026-05-01T10:00:00Z",
                                    "status": {other_pr}
                                }},
                                {{
                                    "number": 7,
                                    "attached_at": "2026-05-01T08:00:00Z",
                                    "status": null
                                }}
                            ]
                        }}
                    ]
                }}
            ]
        }}"#,
            branch_pr = pr(7, "feat/thing"),
            other_pr = pr(8, "feat/stacked"),
        )
    }

    #[tokio::test]
    async fn boot_folds_both_pr_slots_into_one_list() {
        let f = fixture_with_raw(&two_slot_state_json()).await;
        let link = f
            .store
            .read(|s| s.workspaces[0].repo_links[0].clone())
            .await;
        assert_eq!(
            link.prs.iter().map(|p| p.number).collect::<Vec<_>>(),
            vec![7, 8]
        );
        assert_eq!(link.prs[0].status.as_ref().unwrap().pr_number, 7);
        assert_eq!(link.prs[1].status.as_ref().unwrap().pr_number, 8);
        assert!(link.dismissed.is_empty());
    }

    #[tokio::test]
    async fn a_migrated_file_is_not_migrated_again() {
        let raw = r#"{
            "workspaces": [
                {
                    "id": "ws-0",
                    "branch": "feat/thing",
                    "created_at": "2026-04-01T12:00:00Z",
                    "status": {"kind": "ready"},
                    "repo_links": [
                        {
                            "repo_key": "api",
                            "worktree_path": "/tmp/ws-0/api",
                            "setup_script_ran_at": null,
                            "prs": [{"number": 9, "tracked_at": "2026-05-01T09:00:00Z", "status": null}],
                            "dismissed": [7]
                        }
                    ]
                }
            ]
        }"#;
        let f = fixture_with_raw(raw).await;
        let link = f
            .store
            .read(|s| s.workspaces[0].repo_links[0].clone())
            .await;
        assert_eq!(
            link.prs.iter().map(|p| p.number).collect::<Vec<_>>(),
            vec![9]
        );
        assert_eq!(link.dismissed, vec![7]);
    }

    fn many_sessions_state_json() -> &'static str {
        r#"{
            "workspaces": [
                {
                    "id": "ws-0",
                    "branch": "feat/thing",
                    "created_at": "2026-04-01T12:00:00Z",
                    "status": {"kind": "ready"},
                    "repo_links": [],
                    "sessions": [
                        {
                            "id": "hidden-one",
                            "repo_key": null,
                            "cwd": "/tmp/ws-0",
                            "claude_session_id": "csid-hidden",
                            "transcript_path": null,
                            "hidden": true
                        },
                        {
                            "id": "kept",
                            "repo_key": "api",
                            "cwd": "/tmp/ws-0/api",
                            "claude_session_id": "csid-kept",
                            "transcript_path": null,
                            "claude_binary": "claude-hipaa",
                            "runtime_state": "waiting_input"
                        },
                        {
                            "id": "never-started",
                            "repo_key": null,
                            "cwd": "/tmp/ws-0",
                            "claude_session_id": null,
                            "transcript_path": null
                        }
                    ]
                }
            ]
        }"#
    }

    #[tokio::test]
    async fn boot_keeps_the_newest_visible_session_with_a_conversation() {
        let f = fixture_with_raw(many_sessions_state_json()).await;
        let ws = f.store.read(|s| s.workspaces[0].clone()).await;
        let session = ws.session.expect("one session survives");
        assert_eq!(session.id, "kept");
        assert_eq!(session.agent_session_id.as_deref(), Some("csid-kept"));
        assert_eq!(session.cwd, PathBuf::from("/tmp/ws-0/api"));
        assert_eq!(
            session.runtime_state,
            Some(crate::state::SessionRuntimeState::WaitingInput)
        );
        assert_eq!(ws.agent_binary.as_deref(), Some("claude-hipaa"));
    }

    #[tokio::test]
    async fn a_file_with_one_session_is_not_migrated_again() {
        let raw = r#"{
            "workspaces": [
                {
                    "id": "ws-0",
                    "branch": "feat/thing",
                    "created_at": "2026-04-01T12:00:00Z",
                    "status": {"kind": "ready"},
                    "repo_links": [],
                    "session": {
                        "id": "current",
                        "cwd": "/tmp/ws-0",
                        "claude_session_id": null,
                        "transcript_path": null
                    },
                    "sessions": [
                        {
                            "id": "stale",
                            "cwd": "/tmp/ws-0",
                            "claude_session_id": "csid",
                            "transcript_path": null
                        }
                    ]
                }
            ]
        }"#;
        let f = fixture_with_raw(raw).await;
        let session = f.store.read(|s| s.workspaces[0].session.clone()).await;
        assert_eq!(session.map(|s| s.id).as_deref(), Some("current"));
    }

    #[tokio::test]
    async fn nothing_archived_means_no_folder() {
        let state = AppState {
            workspaces: vec![workspace("ws-1", WorkspaceStatus::Ready)],
            ..Default::default()
        };
        let f = fixture_with(Some(state)).await;
        assert!(f.store.read(|s| s.folders.is_empty()).await);
    }

    #[tokio::test]
    async fn boot_sends_workspaces_in_missing_folders_back_to_default() {
        let mut ws = workspace("ws-1", WorkspaceStatus::Ready);
        ws.folder = Some("folder-that-went-away".into());
        let state = AppState {
            workspaces: vec![ws],
            ..Default::default()
        };
        let f = fixture_with(Some(state)).await;

        assert_eq!(
            f.store.with_workspace("ws-1", |w| w.folder.clone()).await.unwrap(),
            None
        );
    }
}


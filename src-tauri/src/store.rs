use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tokio::sync::{Notify, RwLock};
use tracing::{debug, error, info, warn};

use crate::error::{AppError, AppResult};
use crate::github::GithubPrStatus;
use crate::state::{AppState, Folder, Workspace, WorkspaceStatus};

/// Where the `Store` announces that a workspace changed so the UI can refresh.
///
/// This exists so `Store` can own "persist and notify" as one operation
/// without depending on Tauri: the app passes an `AppHandle` adapter, tests
/// pass a recording one. Two adapters, so it's a real seam.
pub trait WorkspaceNotifier: Send + Sync + 'static {
    fn workspace_changed(&self, workspace_id: &str);
}

/// Notifier for contexts with no UI attached.
#[cfg(test)]
pub struct NullNotifier;

#[cfg(test)]
impl WorkspaceNotifier for NullNotifier {
    fn workspace_changed(&self, _workspace_id: &str) {}
}

/// The source of truth for Tethys workspace state.
///
/// Writes go through `mutate`, which applies the closure under a write lock
/// and nudges a background flusher. The flusher coalesces bursts of writes
/// (~250ms debounce) into a single atomic temp-file + rename.
pub struct Store {
    state: Arc<RwLock<AppState>>,
    dirty: Arc<Notify>,
    state_path: PathBuf,
    tmp_path: PathBuf,
    notifier: Box<dyn WorkspaceNotifier>,
}

const DEBOUNCE: Duration = Duration::from_millis(250);

impl Store {
    /// Load `state.json` (or initialize an empty state), then start the background flusher.
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
        }

        let stranded = initial.prune_missing_folders();
        if stranded > 0 {
            warn!(
                count = stranded,
                "workspaces named a folder that doesn't exist; moved to Default"
            );
        }

        // A `Creating` entry means the previous run crashed mid-provision;
        // a `CreationFailed` entry means the user never dismissed it before
        // shutdown. Either way the in-memory progress events that drove the
        // log pane are gone, so the row is dead UI — drop it. The boot-time
        // reconciler picks up any worktree directories left on disk.
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
            // A pruned draft can still be somebody's blocker: an agent handing
            // off with `blocks_caller` points the caller at a workspace that is
            // a `Creating` draft for as long as provisioning takes. Once the
            // draft is gone the id means nothing, so drop the link rather than
            // leave a pointer nothing can resolve.
            for id in &pruned {
                initial.clear_links_to(id);
            }
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

    /// Read-only access to the state.
    pub async fn read<R, F: FnOnce(&AppState) -> R>(&self, f: F) -> R {
        let guard = self.state.read().await;
        f(&guard)
    }

    /// Apply a mutation under a write lock and schedule a flush.
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

    /// Read one workspace. `WorkspaceNotFound` when it's gone.
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

    /// Mutate one workspace, persist, and tell the UI.
    ///
    /// This is the operation almost every caller actually wants — 21 of the 22
    /// original `mutate` call sites looked up a single workspace and then had
    /// to remember three separate unwritten rules: which lookup idiom to use,
    /// whether a missing workspace is an error or a no-op, and to emit
    /// `workspace:changed` afterwards. All three now live here.
    ///
    /// `WorkspaceNotFound` if the workspace is gone; nothing is persisted and
    /// no notification is sent in that case.
    pub async fn update_workspace<R, F>(&self, id: &str, f: F) -> AppResult<R>
    where
        F: FnOnce(&mut Workspace) -> AppResult<R>,
    {
        let result = self.update_workspace_quiet(id, f).await?;
        self.notifier.workspace_changed(id);
        Ok(result)
    }

    /// Announce a workspace as changed without editing it.
    ///
    /// For writes that went through [`Store::mutate`], which spans many
    /// workspaces and so can't notify per workspace on its own. The poller
    /// needs it: its `status_changed` event can only update a PR the frontend
    /// already knows about, so a tick that starts *tracking* a PR has to tell
    /// the UI the list itself grew.
    pub fn notify_workspace_changed(&self, id: &str) {
        self.notifier.workspace_changed(id);
    }

    /// [`Store::update_workspace`] without the notification.
    ///
    /// For edits the UI already shows locally and re-renders on: notes typing,
    /// where echoing every keystroke back would fight the user's cursor.
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

    /// Announce a workspace change the store didn't make itself — used where
    /// the interesting edit lives outside `AppState` (a supervisor's in-memory
    /// map) but the UI still needs to re-read the workspace.
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

        // Best-effort fsync on the temp file before rename.
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

/// Fold the retired `archived_at` marker into an ordinary folder named
/// "Archived".
///
/// Reads the field straight out of the raw JSON rather than off `Workspace`,
/// so the type carries no trace of a concept that no longer exists. The first
/// flush after this rewrites the file without the field, which makes every
/// later boot a no-op — there is nothing to keep, and nothing to undo.
///
/// The folder starts collapsed, because that is how the archive drawer it
/// replaces always looked on launch.
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

/// Fold the retired two-slot PR layout — `github` for the workspace's own
/// branch, `attached_prs` for everything else — into `RepoLink::prs`.
///
/// Same shape as [`migrate_archived_to_folder`], and for the same reason: read
/// the old fields off the raw JSON so `RepoLink` carries no trace of a split
/// that no longer exists, and let the first flush end the migration for good.
///
/// The branch PR goes first, which is where it has always been drawn. It is
/// also the one entry that can collide: a PR could sit in `github` and in
/// `attached_prs` at once if it was hand-attached just before the poller found
/// it by branch, so `track` is what merges them rather than a plain push.
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
            // Only ever fills an empty list: a file already carrying `prs` has
            // been through this, and its old fields are gone.
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

/// The retired `attached_prs` entry, read only by [`migrate_pr_slots`]. Named
/// apart from `TrackedPr` because the field it keyed on was `attached_at`.
#[derive(Deserialize)]
struct OldAttachedPr {
    number: u32,
    #[serde(default)]
    status: Option<GithubPrStatus>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Origin, Workspace, WorkspaceStatus};
    use chrono::Utc;
    use std::sync::Mutex;

    /// Records the workspace ids the store announced, so tests can assert on
    /// the notification half of `update_workspace`'s contract.
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
            sessions: Vec::new(),
            claude_binary: None,
            origin: Origin::Ui,
            deleted_at: None,
            folder: None,
            status,
            script_runs: Vec::new(),
            notes: String::new(),
            blocked_by: None,
        }
    }

    struct Fixture {
        store: Arc<Store>,
        notified: Arc<Mutex<Vec<String>>>,
        _tmp: tempfile::TempDir,
    }

    async fn fixture_with(state: Option<AppState>) -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let state_path = tmp.path().join("state.json");
        if let Some(state) = state {
            std::fs::write(&state_path, serde_json::to_vec(&state).unwrap()).unwrap();
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
        fixture_with(None).await
    }

    /// Boots from literal file contents, for the migration — whose whole job is
    /// reading a field `Workspace` no longer has.
    async fn fixture_with_raw(raw: &str) -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let state_path = tmp.path().join("state.json");
        std::fs::write(&state_path, raw).unwrap();
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

    /// A `Creating` row means the previous run died mid-provision and a
    /// `CreationFailed` row was never dismissed. Both are dead UI on the next
    /// boot — the progress events that drove their log pane are gone.
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

    /// The whole point of the workspace-scoped interface: lookup, not-found
    /// policy, persist, and notify are one operation rather than four things
    /// each caller has to remember.
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

    /// A closure that fails must leave no trace: no persisted edit, and no
    /// notification telling the UI to re-read something that didn't change.
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

    /// Notes are typed character by character with the frontend holding the
    /// authoritative text; echoing every keystroke back would fight the cursor.
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

    /// state.json is written by temp-file + rename so a crash mid-write can't
    /// truncate it. Verify the bytes actually land at the real path.
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

    /// Malformed JSON must not take the app down with it — the user gets an
    /// empty state and a loud log line rather than a crash loop.
    #[tokio::test]
    async fn unparseable_state_json_starts_empty_instead_of_failing() {
        let tmp = tempfile::tempdir().unwrap();
        let state_path = tmp.path().join("state.json");
        std::fs::write(&state_path, b"{ this is not json").unwrap();

        let store = Store::load(
            state_path,
            tmp.path().join("state.json.tmp"),
            Box::new(NullNotifier),
        )
        .await
        .unwrap();
        assert_eq!(store.read(|s| s.workspaces.len()).await, 0);
    }

    /// The archive marker retires into an ordinary folder. It starts collapsed
    /// because that is how the drawer it replaces always looked on launch.
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

    /// The migration is driven by the file, so the first flush without the
    /// field ends it. A second boot must not mint an empty second "Archived".
    #[tokio::test]
    async fn the_migration_does_not_run_twice() {
        let f = fixture_with_raw(&archived_state_json()).await;
        let migrated = f.store.read(|s| s.clone()).await;

        let again = fixture_with(Some(migrated)).await;
        let folders = again.store.read(|s| s.folders.clone()).await;
        assert_eq!(folders.len(), 1, "one Archived folder, not two");
    }

    /// A `state.json` from the two-slot era: PR 7 in the branch slot, PR 8
    /// attached by hand, and PR 7 *also* still sitting in `attached_prs` from
    /// before the poller caught up.
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
        // The branch PR keeps its place at the front, and the duplicate copy of
        // it that was in `attached_prs` collapses into the same entry.
        assert_eq!(
            link.prs.iter().map(|p| p.number).collect::<Vec<_>>(),
            vec![7, 8]
        );
        assert_eq!(link.prs[0].status.as_ref().unwrap().pr_number, 7);
        assert_eq!(link.prs[1].status.as_ref().unwrap().pr_number, 8);
        assert!(link.dismissed.is_empty());
    }

    /// The first flush drops the old fields, so every later boot is a no-op —
    /// and a file that already carries `prs` is left exactly as it is.
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

    #[tokio::test]
    async fn nothing_archived_means_no_folder() {
        let state = AppState {
            workspaces: vec![workspace("ws-1", WorkspaceStatus::Ready)],
            ..Default::default()
        };
        let f = fixture_with(Some(state)).await;
        assert!(f.store.read(|s| s.folders.is_empty()).await);
    }

    /// `state.json` is hand-editable, so a workspace can name a folder that
    /// isn't there. It lands in Default rather than dropping off the sidebar.
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


use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Notify, RwLock};
use tracing::{debug, error, info, warn};

use crate::error::{AppError, AppResult};
use crate::state::{AppState, Workspace, WorkspaceStatus};

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
        let mut initial = match tokio::fs::read(&state_path).await {
            Ok(bytes) if !bytes.is_empty() => match serde_json::from_slice::<AppState>(&bytes) {
                Ok(s) => {
                    info!(workspaces = s.workspaces.len(), "loaded state.json");
                    s
                }
                Err(e) => {
                    error!(error = %e, "state.json failed to parse; starting empty");
                    AppState::default()
                }
            },
            Ok(_) => AppState::default(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                info!("no state.json yet; starting empty");
                AppState::default()
            }
            Err(e) => return Err(e.into()),
        };

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
            archived_at: None,
            status,
            script_runs: Vec::new(),
            notes: String::new(),
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
            system_errors: Vec::new(),
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
            system_errors: Vec::new(),
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
            system_errors: Vec::new(),
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
            system_errors: Vec::new(),
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
}

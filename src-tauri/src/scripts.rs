use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use serde::Serialize;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter};
use tracing::info;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::pty::{OnExit, PtyProcess, PtySpawn};
use crate::state::ScriptRunId;
use crate::store::Store;
use crate::tmux;

const RING_CAPACITY: usize = 1024 * 1024;

/// Snapshot returned to the frontend. Live PTY bytes stream through a
/// `Channel` via `attach`.
#[derive(Debug, Clone, Serialize)]
pub struct ScriptInfo {
    pub id: ScriptRunId,
    pub workspace_id: String,
    pub repo_key: String,
    pub script_name: String,
    pub command: String,
    pub cwd: PathBuf,
    pub running: bool,
    pub started_at: DateTime<Utc>,
}

struct ScriptHandle {
    info: ScriptInfo,
    pty: PtyProcess,
}

struct SpawnRequest<'a> {
    id: ScriptRunId,
    workspace_id: String,
    repo_key: String,
    script_name: String,
    command: String,
    cwd: &'a Path,
    tmux_bin: &'a Path,
    tmux_args: &'a [String],
    seed_bytes: &'a [u8],
    started_at: DateTime<Utc>,
}

pub struct ScriptSupervisor {
    scripts: Mutex<HashMap<ScriptRunId, ScriptHandle>>,
    store: Arc<Store>,
    app: AppHandle,
}

impl ScriptSupervisor {
    pub fn new(app: AppHandle, store: Arc<Store>) -> Self {
        Self {
            scripts: Mutex::new(HashMap::new()),
            store,
            app,
        }
    }

    /// Spawn `command` in `cwd` inside a fresh tmux session on the Tethys
    /// server. The command is run via `/bin/zsh -lc <command>` so the user's
    /// login PATH is set up (Homebrew etc.) before the actual program runs.
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        &self,
        workspace_id: String,
        repo_key: String,
        script_name: String,
        command: String,
        cwd: &Path,
        tmux_bin: &Path,
    ) -> AppResult<ScriptInfo> {
        let id = Uuid::new_v4().to_string();
        let started_at = Utc::now();

        let mut args: Vec<String> = vec!["-L".into(), tmux::SOCKET_LABEL.into()];
        args.extend(tmux::server_init_args());
        args.extend([
            "new-session".into(),
            "-A".into(),
            "-D".into(),
            "-s".into(),
            id.clone(),
            "-x".into(),
            "200".into(),
            "-y".into(),
            "50".into(),
            "--".into(),
            "/bin/zsh".into(),
            "-lc".into(),
            command.clone(),
        ]);

        self.spawn_with_id(SpawnRequest {
            id,
            workspace_id,
            repo_key,
            script_name,
            command,
            cwd,
            tmux_bin,
            tmux_args: &args,
            seed_bytes: &[],
            started_at,
        })
    }

    /// Reattach to a script's existing tmux session at boot, replaying the
    /// captured scrollback so logs are preserved.
    #[allow(clippy::too_many_arguments)]
    pub fn reattach_tmux(
        &self,
        id: ScriptRunId,
        workspace_id: String,
        repo_key: String,
        script_name: String,
        command: String,
        cwd: &Path,
        tmux_bin: &Path,
        started_at: DateTime<Utc>,
    ) -> AppResult<ScriptInfo> {
        if !tmux::has_session(tmux_bin, &id) {
            return Err(AppError::Other(format!(
                "tmux session {id} no longer exists"
            )));
        }
        let seed = tmux::capture_pane(tmux_bin, &id).unwrap_or_default();
        let mut args: Vec<String> = vec!["-L".into(), tmux::SOCKET_LABEL.into()];
        args.extend(tmux::server_init_args());
        args.extend([
            "attach-session".into(),
            "-d".into(),
            "-t".into(),
            id.clone(),
        ]);
        self.spawn_with_id(SpawnRequest {
            id,
            workspace_id,
            repo_key,
            script_name,
            command,
            cwd,
            tmux_bin,
            tmux_args: &args,
            seed_bytes: &seed,
            started_at,
        })
    }

    fn spawn_with_id(&self, req: SpawnRequest<'_>) -> AppResult<ScriptInfo> {
        let SpawnRequest {
            id,
            workspace_id,
            repo_key,
            script_name,
            command,
            cwd,
            tmux_bin,
            tmux_args,
            seed_bytes,
            started_at,
        } = req;

        let info = ScriptInfo {
            id: id.clone(),
            workspace_id: workspace_id.clone(),
            repo_key,
            script_name,
            command,
            cwd: cwd.to_path_buf(),
            running: true,
            started_at,
        };

        let pty = PtyProcess::spawn(
            PtySpawn {
                program: tmux_bin,
                args: tmux_args,
                cwd,
                seed_bytes,
                ring_capacity: RING_CAPACITY,
                tmux_session_name: id.clone(),
                tmux_bin: tmux_bin.to_path_buf(),
            },
            script_exit_hook(
                self.app.clone(),
                self.store.clone(),
                workspace_id.clone(),
                id.clone(),
            ),
        )?;

        let handle = ScriptHandle {
            info: info.clone(),
            pty,
        };
        self.scripts.lock().unwrap().insert(id, handle);
        let _ = self.app.emit(
            "script:changed",
            serde_json::json!({ "workspace_id": workspace_id }),
        );
        Ok(info)
    }

    /// Register a new output subscriber and return the current scrollback.
    pub fn attach(
        &self,
        script_id: &str,
        channel: Channel<InvokeResponseBody>,
    ) -> AppResult<Vec<u8>> {
        let scripts = self.scripts.lock().unwrap();
        let handle = scripts
            .get(script_id)
            .ok_or_else(|| AppError::Other(format!("script not found: {script_id}")))?;
        Ok(handle.pty.attach(channel))
    }

    pub fn send_input(&self, script_id: &str, data: &[u8]) -> AppResult<()> {
        let scripts = self.scripts.lock().unwrap();
        scripts
            .get(script_id)
            .ok_or_else(|| AppError::Other(format!("script not found: {script_id}")))?
            .pty
            .send_input(data)
    }

    pub fn resize(&self, script_id: &str, cols: u16, rows: u16) -> AppResult<()> {
        let scripts = self.scripts.lock().unwrap();
        scripts
            .get(script_id)
            .ok_or_else(|| AppError::Other(format!("script not found: {script_id}")))?
            .pty
            .resize(cols, rows)
    }

    pub fn list_for_workspace(&self, workspace_id: &str) -> Vec<ScriptInfo> {
        let scripts = self.scripts.lock().unwrap();
        scripts
            .values()
            .filter(|h| h.info.workspace_id == workspace_id)
            .map(|h| {
                let mut info = h.info.clone();
                info.running = h.pty.is_running();
                info
            })
            .collect()
    }

    /// Kill the tmux session and drop the in-memory handle. Used both by the
    /// chip's × button (cancel + forget in one step) and when a fresh start
    /// needs to clear out a prior run for the same `(repo, script_name)`.
    pub fn dismiss(&self, script_id: &str, tmux_bin: &Path) {
        tmux::kill_session(tmux_bin, script_id);
        self.scripts.lock().unwrap().remove(script_id);
    }

    /// Kill every script attached to `workspace_id`. Called inline from
    /// `delete_workspace` so dev servers stop writing to the worktree before
    /// the purger removes it.
    pub fn kill_for_workspace(&self, workspace_id: &str, tmux_bin: &Path) {
        let ids: Vec<String> = {
            let scripts = self.scripts.lock().unwrap();
            scripts
                .values()
                .filter(|h| h.info.workspace_id == workspace_id)
                .map(|h| h.info.id.clone())
                .collect()
        };
        for id in ids {
            tmux::kill_session(tmux_bin, &id);
        }
    }
}

/// Build the exit hook handed to [`PtyProcess::spawn`]. It runs only on a
/// true child exit (the watcher already filtered out client detaches): emit
/// `script:exit` and prune the run from persisted state. The ring is unused —
/// scripts have no detach epilogue to scrub.
fn script_exit_hook(
    app: AppHandle,
    store: Arc<Store>,
    workspace_id: String,
    script_id: ScriptRunId,
) -> OnExit {
    Box::new(move |code, _ring| {
        info!(%script_id, ?code, "script process exited");
        let _ = app.emit(
            "script:exit",
            serde_json::json!({
                "workspace_id": workspace_id,
                "script_id": script_id,
                "code": code,
            }),
        );

        let workspace_id_for_state = workspace_id.clone();
        let id_for_state = script_id.clone();
        tauri::async_runtime::spawn(async move {
            let _ = store
                .mutate(|s| {
                    if let Some(ws) = s.find_workspace_mut(&workspace_id_for_state) {
                        ws.script_runs.retain(|m| m.id != id_for_state);
                    }
                    Ok(())
                })
                .await;
        });
    })
}

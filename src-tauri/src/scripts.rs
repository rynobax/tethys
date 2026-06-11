use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::state::ScriptRunId;
use crate::store::Store;
use crate::tmux;

const RING_CAPACITY: usize = 1024 * 1024;
const READ_BUF: usize = 4096;

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
    #[allow(dead_code)]
    master: Box<dyn MasterPty + Send>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    ring: Arc<Mutex<VecDeque<u8>>>,
    subscribers: Arc<Mutex<Vec<Channel<InvokeResponseBody>>>>,
    running: Arc<Mutex<bool>>,
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

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: 30,
                cols: 100,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| AppError::Other(format!("openpty failed: {e}")))?;

        let mut cmd = CommandBuilder::new(tmux_bin);
        for arg in tmux_args {
            cmd.arg(arg);
        }
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");
        crate::child_env::sanitize_for_child_repo(&mut cmd);

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| AppError::Other(format!("spawn failed: {e}")))?;
        drop(pair.slave);

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| AppError::Other(format!("clone reader failed: {e}")))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| AppError::Other(format!("take writer failed: {e}")))?;

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

        let ring = Arc::new(Mutex::new(VecDeque::with_capacity(RING_CAPACITY)));
        if !seed_bytes.is_empty() {
            append_to_ring(&ring, seed_bytes);
        }
        let subscribers: Arc<Mutex<Vec<Channel<InvokeResponseBody>>>> =
            Arc::new(Mutex::new(Vec::new()));
        let running = Arc::new(Mutex::new(true));

        spawn_reader_thread(reader, ring.clone(), subscribers.clone());
        spawn_child_watcher(
            child,
            id.clone(),
            workspace_id.clone(),
            running.clone(),
            tmux_bin.to_path_buf(),
            self.app.clone(),
            self.store.clone(),
        );

        let handle = ScriptHandle {
            info: info.clone(),
            master: pair.master,
            writer: Arc::new(Mutex::new(writer)),
            ring,
            subscribers,
            running,
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
        let scrollback: Vec<u8> = handle.ring.lock().unwrap().iter().copied().collect();
        handle.subscribers.lock().unwrap().push(channel);
        Ok(scrollback)
    }

    pub fn send_input(&self, script_id: &str, data: &[u8]) -> AppResult<()> {
        let writer = {
            let scripts = self.scripts.lock().unwrap();
            scripts
                .get(script_id)
                .ok_or_else(|| AppError::Other(format!("script not found: {script_id}")))?
                .writer
                .clone()
        };
        writer
            .lock()
            .unwrap()
            .write_all(data)
            .map_err(|e| AppError::Other(format!("write: {e}")))?;
        Ok(())
    }

    pub fn resize(&self, script_id: &str, cols: u16, rows: u16) -> AppResult<()> {
        let scripts = self.scripts.lock().unwrap();
        let handle = scripts
            .get(script_id)
            .ok_or_else(|| AppError::Other(format!("script not found: {script_id}")))?;
        handle
            .master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| AppError::Other(format!("resize: {e}")))?;
        Ok(())
    }

    pub fn list_for_workspace(&self, workspace_id: &str) -> Vec<ScriptInfo> {
        let scripts = self.scripts.lock().unwrap();
        scripts
            .values()
            .filter(|h| h.info.workspace_id == workspace_id)
            .map(|h| {
                let mut info = h.info.clone();
                info.running = *h.running.lock().unwrap();
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

fn append_to_ring(ring: &Arc<Mutex<VecDeque<u8>>>, data: &[u8]) {
    let mut ring = ring.lock().unwrap();
    if data.len() >= RING_CAPACITY {
        ring.clear();
        ring.extend(&data[data.len() - RING_CAPACITY..]);
        return;
    }
    let overflow = (ring.len() + data.len()).saturating_sub(RING_CAPACITY);
    for _ in 0..overflow {
        ring.pop_front();
    }
    ring.extend(data.iter().copied());
}

fn spawn_reader_thread(
    mut reader: Box<dyn Read + Send>,
    ring: Arc<Mutex<VecDeque<u8>>>,
    subscribers: Arc<Mutex<Vec<Channel<InvokeResponseBody>>>>,
) {
    std::thread::spawn(move || {
        let mut buf = [0u8; READ_BUF];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    debug!("script reader: EOF");
                    break;
                }
                Ok(n) => {
                    let chunk = &buf[..n];
                    append_to_ring(&ring, chunk);
                    let mut subs = subscribers.lock().unwrap();
                    subs.retain(|sub| sub.send(InvokeResponseBody::Raw(chunk.to_vec())).is_ok());
                }
                Err(e) => {
                    warn!(error = %e, "script reader error");
                    break;
                }
            }
        }
    });
}

fn spawn_child_watcher(
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
    script_id: ScriptRunId,
    workspace_id: String,
    running: Arc<Mutex<bool>>,
    tmux_bin: PathBuf,
    app: AppHandle,
    store: Arc<Store>,
) {
    std::thread::spawn(move || {
        let status = child.wait();
        *running.lock().unwrap() = false;
        let code = status.ok().map(|s| s.exit_code() as i32);

        if tmux::has_session(&tmux_bin, &script_id) {
            info!(%script_id, ?code, "script tmux client exited but session still alive");
            return;
        }

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
    });
}

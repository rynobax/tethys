use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::hook_listener::HookMessage;
use crate::state::SessionRuntimeState;
use crate::store::Store;
use crate::tmux;

const RING_CAPACITY: usize = 2 * 1024 * 1024; // 2 MB scrollback per session
const READ_BUF: usize = 4096;

/// Inputs to `SessionSupervisor::spawn_with_id`. Bundled in a struct so the
/// inner function doesn't trip clippy's `too_many_arguments` lint.
struct SpawnRequest<'a> {
    id: SessionId,
    workspace_id: String,
    repo_key: Option<String>,
    cwd: &'a Path,
    program: &'a Path,
    args: &'a [String],
    tmux_bin: PathBuf,
    seed_bytes: &'a [u8],
}

pub type SessionId = String;

/// Snapshot returned to the frontend for the sessions list. Does not include
/// the live byte stream — that flows over a `Channel` via `attach`.
#[derive(Debug, Clone, Serialize)]
pub struct SessionInfo {
    pub id: SessionId,
    pub workspace_id: String,
    /// `None` => session is rooted at the workspace's parent dir (which
    /// contains every repo subdir), not inside any one repo.
    pub repo_key: Option<String>,
    pub cwd: PathBuf,
    pub running: bool,
    pub runtime_state: SessionRuntimeState,
    /// Populated by the last Notification hook (e.g. `permission_prompt`).
    /// Set to `None` when state transitions away from `WaitingInput`.
    pub notification_type: Option<String>,
    /// User dismissed the "your turn" dot for this session. Reset on the
    /// next runtime_state transition.
    pub turn_acknowledged: bool,
}

struct SessionHandle {
    info: SessionInfo,
    master: Box<dyn MasterPty + Send>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    ring: Arc<Mutex<VecDeque<u8>>>,
    /// Fan-out targets for live PTY bytes. Writers that error (client closed)
    /// are dropped on the next tick.
    subscribers: Arc<Mutex<Vec<Channel<InvokeResponseBody>>>>,
    /// Flipped to `false` when the child process exits.
    running: Arc<Mutex<bool>>,
}

/// One entry per in-flight Claude spawn awaiting its `SessionStart` hook.
/// Cleaned up when the hook arrives or when the entry expires.
struct PendingSpawn {
    workspace_id: String,
    session_id: SessionId,
    expires_at: Instant,
}

const PENDING_TTL: Duration = Duration::from_secs(30);

/// Per-session UI state (turn + last notification subtype + the user's
/// dismissal of the "your turn" dot). Held in memory; the persisted
/// mirror lives on `ClaudeSessionMeta` so all three survive restarts.
#[derive(Debug, Default, Clone)]
struct TurnState {
    state: SessionRuntimeState,
    notification_type: Option<String>,
    acknowledged: bool,
}

pub struct SessionSupervisor {
    sessions: Mutex<HashMap<SessionId, SessionHandle>>,
    /// Maps the `TETHYS_SPAWN_TOKEN` we set on the PTY env to the
    /// session metadata we need to update once Claude's SessionStart hook
    /// tells us the claude_session_id.
    pending: Mutex<HashMap<String, PendingSpawn>>,
    /// Per-session turn state. Keyed by Tethys `SessionId`.
    turn: Mutex<HashMap<SessionId, TurnState>>,
    store: Arc<Store>,
    app: AppHandle,
}

impl SessionSupervisor {
    pub fn new(app: AppHandle, store: Arc<Store>) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            turn: Mutex::new(HashMap::new()),
            store,
            app,
        }
    }

    /// Seed the in-memory turn map from a persisted snapshot. Used at
    /// boot, immediately after `reattach_tmux` clobbers the entry with
    /// `Working`. Does not persist (the value came from disk) and does
    /// not emit — the frontend hasn't subscribed yet, and `list_sessions`
    /// will pick the value up from `list_for_workspace`.
    pub fn seed_turn(
        &self,
        session_id: &str,
        state: SessionRuntimeState,
        notification_type: Option<String>,
        acknowledged: bool,
    ) {
        let mut map = self.turn.lock().unwrap();
        map.insert(
            session_id.to_string(),
            TurnState {
                state,
                notification_type,
                acknowledged,
            },
        );
    }

    /// Update a session's turn state + emit `session:turn_changed` + write
    /// the new state through to `state.json` so the indicator survives a
    /// Tethys restart. No-op if the new state matches the current one.
    async fn set_turn(
        &self,
        session_id: &str,
        workspace_id: &str,
        state: SessionRuntimeState,
        notification_type: Option<String>,
    ) {
        let changed = {
            let mut map = self.turn.lock().unwrap();
            let current = map.entry(session_id.to_string()).or_default();
            if current.state == state && current.notification_type == notification_type {
                false
            } else {
                current.state = state;
                current.notification_type = notification_type.clone();
                // A state transition is the user-facing signal that something
                // fresh happened — re-light any dismissed indicator.
                current.acknowledged = false;
                true
            }
        };
        if !changed {
            return;
        }
        let _ = self.app.emit(
            "session:turn_changed",
            serde_json::json!({
                "workspace_id": workspace_id,
                "session_id": session_id,
                "runtime_state": state,
                "notification_type": notification_type,
                "turn_acknowledged": false,
            }),
        );
        let persist = self
            .store
            .mutate(|s| {
                if let Some(ws) = s.find_workspace_mut(workspace_id) {
                    if let Some(meta) = ws.sessions.iter_mut().find(|m| m.id == session_id) {
                        meta.runtime_state = Some(state);
                        meta.notification_type = notification_type.clone();
                        meta.turn_acknowledged = false;
                    }
                }
                Ok(())
            })
            .await;
        if let Err(e) = persist {
            warn!(error = %e, session_id, "persist turn state failed");
        }
    }

    /// User dismissed the "your turn" indicator. Sets `turn_acknowledged`
    /// in memory, persists it, and emits a `session:turn_changed` event so
    /// the sidebar dot vanishes immediately. The flag is cleared again on
    /// the next runtime_state transition (see `set_turn`).
    pub async fn acknowledge_turn(
        &self,
        session_id: &str,
        workspace_id: &str,
    ) -> AppResult<()> {
        let (state, notification_type) = {
            let mut map = self.turn.lock().unwrap();
            let current = map.entry(session_id.to_string()).or_default();
            if current.acknowledged {
                return Ok(());
            }
            current.acknowledged = true;
            (current.state, current.notification_type.clone())
        };
        let _ = self.app.emit(
            "session:turn_changed",
            serde_json::json!({
                "workspace_id": workspace_id,
                "session_id": session_id,
                "runtime_state": state,
                "notification_type": notification_type,
                "turn_acknowledged": true,
            }),
        );
        self.store
            .mutate(|s| {
                if let Some(ws) = s.find_workspace_mut(workspace_id) {
                    if let Some(meta) = ws.sessions.iter_mut().find(|m| m.id == session_id) {
                        meta.turn_acknowledged = true;
                    }
                }
                Ok(())
            })
            .await
    }

    /// Inner spawn: opens a PTY, runs `program args`, wires up reader/
    /// subscribers/watcher, and stores a `SessionHandle` under `id`. The
    /// caller provides `id` so it can match an existing tmux session name
    /// (the tmux session name == Tethys SessionId by convention).
    fn spawn_with_id(&self, req: SpawnRequest<'_>) -> AppResult<SessionInfo> {
        let SpawnRequest {
            id,
            workspace_id,
            repo_key,
            cwd,
            program,
            args,
            tmux_bin,
            seed_bytes,
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

        let mut cmd = CommandBuilder::new(program);
        for arg in args {
            cmd.arg(arg);
        }
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");

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

        let info = SessionInfo {
            id: id.clone(),
            workspace_id: workspace_id.clone(),
            repo_key,
            cwd: cwd.to_path_buf(),
            running: true,
            runtime_state: SessionRuntimeState::Working,
            notification_type: None,
            turn_acknowledged: false,
        };

        let ring = Arc::new(Mutex::new(VecDeque::with_capacity(RING_CAPACITY)));
        if !seed_bytes.is_empty() {
            // Seed the ring before the reader thread starts so the first
            // attach sees [seed][tmux's fresh redraw] in that order.
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
            ring.clone(),
            tmux_bin,
            self.app.clone(),
        );

        let handle = SessionHandle {
            info: info.clone(),
            master: pair.master,
            writer: Arc::new(Mutex::new(writer)),
            ring,
            subscribers,
            running,
        };

        self.sessions.lock().unwrap().insert(id.clone(), handle);
        // Default to Working — a just-spawned/attached Claude is likely
        // doing something (starting up, or mid-response after we reattach).
        // Hooks will refine shortly.
        self.turn.lock().unwrap().insert(
            id,
            TurnState {
                state: SessionRuntimeState::Working,
                notification_type: None,
                acknowledged: false,
            },
        );
        let _ = self.app.emit(
            "session:changed",
            serde_json::json!({ "workspace_id": workspace_id }),
        );
        Ok(info)
    }

    /// Spawn `claude` inside a fresh tmux session. The tmux server (socket
    /// label `tethys`) keeps the claude process alive across Tethys
    /// restarts — it only dies on reboot, explicit kill, or claude itself
    /// exiting. Pass `resume_claude_session_id` to resume an existing
    /// conversation (`claude --resume <id>`).
    ///
    /// The `TETHYS_SPAWN_TOKEN` correlation var reaches claude via tmux's
    /// `-e` flag (per-session env), so the SessionStart hook still maps
    /// back to the right Tethys session.
    pub fn spawn_claude(
        &self,
        workspace_id: String,
        repo_key: Option<String>,
        cwd: &Path,
        tmux_bin: &Path,
        claude_bin: &Path,
        resume_claude_session_id: Option<&str>,
    ) -> AppResult<(SessionInfo, String)> {
        let token = Uuid::new_v4().to_string();
        let id = new_session_id();

        // Chain: -L <socket> set-option ... ; set-option ... ; new-session ...
        // The server options are prepended so they apply on cold-start
        // (when new-session is what boots the server).
        let mut args: Vec<String> = vec!["-L".into(), tmux::SOCKET_LABEL.into()];
        args.extend(tmux::server_init_args());
        args.extend([
            "new-session".into(),
            "-A".into(), // attach if a session with this name somehow exists
            "-D".into(), // ...and detach any other clients on that session
            "-s".into(),
            id.clone(),
            "-e".into(),
            format!("TETHYS_SPAWN_TOKEN={token}"),
            "-x".into(),
            "200".into(),
            "-y".into(),
            "50".into(),
            "--".into(),
            claude_bin.to_string_lossy().into_owned(),
        ]);
        if let Some(csid) = resume_claude_session_id {
            args.push("--resume".into());
            args.push(csid.to_string());
        }

        let info = self.spawn_with_id(SpawnRequest {
            id,
            workspace_id: workspace_id.clone(),
            repo_key,
            cwd,
            program: tmux_bin,
            args: &args,
            tmux_bin: tmux_bin.to_path_buf(),
            seed_bytes: &[],
        })?;

        // Prune any expired pending correlations while we're here.
        let mut pending = self.pending.lock().unwrap();
        let now = Instant::now();
        pending.retain(|_, p| p.expires_at > now);
        pending.insert(
            token.clone(),
            PendingSpawn {
                workspace_id,
                session_id: info.id.clone(),
                expires_at: now + PENDING_TTL,
            },
        );

        Ok((info, token))
    }

    /// Spawn a non-Claude dev-server inside a fresh tmux session.
    /// Mirrors `spawn_claude` but runs `/bin/zsh -ilc <shell_command>`
    /// as the tmux child process (login + interactive shell so direnv /
    /// yarn / docker resolve), and lets the caller inject per-session
    /// env vars (e.g. `NL_PROXY_TARGET=http://localhost:8001`).
    ///
    /// `session_id` is caller-provided + deterministic
    /// (`tethys-fe-<workspace_id>` / `tethys-be-<workspace_id>`), so the
    /// session can be looked up / killed / reattached without an
    /// external id table.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_dev_server(
        &self,
        session_id: SessionId,
        workspace_id: String,
        repo_key: Option<String>,
        cwd: &Path,
        tmux_bin: &Path,
        shell_command: &str,
        env: &[(String, String)],
    ) -> AppResult<SessionInfo> {
        let mut args: Vec<String> = vec!["-L".into(), tmux::SOCKET_LABEL.into()];
        args.extend(tmux::server_init_args());
        args.extend([
            "new-session".into(),
            "-A".into(), // attach if a session with this name somehow exists
            "-D".into(), // ...and detach any other clients on that session
            "-s".into(),
            session_id.clone(),
            "-x".into(),
            "200".into(),
            "-y".into(),
            "50".into(),
        ]);
        for (k, v) in env {
            args.push("-e".into());
            args.push(format!("{k}={v}"));
        }
        args.extend([
            "--".into(),
            "/bin/zsh".into(),
            "-ilc".into(),
            shell_command.to_string(),
        ]);
        self.spawn_with_id(SpawnRequest {
            id: session_id,
            workspace_id,
            repo_key,
            cwd,
            program: tmux_bin,
            args: &args,
            tmux_bin: tmux_bin.to_path_buf(),
            seed_bytes: &[],
        })
    }

    /// Attach a fresh tmux client to an existing session. Used when the
    /// app restarts and finds the tmux session still alive — claude keeps
    /// running in the tmux server, we just reconnect a new PTY to it.
    /// Returns `AppError` if the tmux session doesn't exist (caller should
    /// fall back to `spawn_claude(..., Some(claude_session_id))`).
    pub fn reattach_tmux(
        &self,
        session_id: SessionId,
        workspace_id: String,
        repo_key: Option<String>,
        cwd: &Path,
        tmux_bin: &Path,
    ) -> AppResult<SessionInfo> {
        if !tmux::has_session(tmux_bin, &session_id) {
            return Err(AppError::Other(format!(
                "tmux session {session_id} no longer exists"
            )));
        }
        // Dump the pane's scrollback before the new client attaches —
        // once the client is attached, tmux will repaint the visible
        // area and we'd lose the historical context in xterm.js.
        let seed = tmux::capture_pane(tmux_bin, &session_id).unwrap_or_default();

        let mut args: Vec<String> = vec!["-L".into(), tmux::SOCKET_LABEL.into()];
        args.extend(tmux::server_init_args());
        args.extend([
            "attach-session".into(),
            "-d".into(), // detach any other clients
            "-t".into(),
            session_id.clone(),
        ]);
        self.spawn_with_id(SpawnRequest {
            id: session_id,
            workspace_id,
            repo_key,
            cwd,
            program: tmux_bin,
            args: &args,
            tmux_bin: tmux_bin.to_path_buf(),
            seed_bytes: &seed,
        })
    }

    /// Dispatch a hook event from `tethys-hook`.
    pub async fn handle_hook_event(&self, msg: HookMessage) {
        match msg.event.as_str() {
            "session-start" => self.handle_session_start(msg).await,
            "user-submit" | "pre-tool" | "post-tool" => {
                self.handle_resume_working(msg).await
            }
            "stop" | "stop-failure" => self.handle_stop(msg).await,
            "notify" => self.handle_notify(msg).await,
            "permission-request" => self.handle_permission_request(msg).await,
            "elicitation" => self.handle_elicitation(msg).await,
            other => debug!(event = %other, "unknown hook event"),
        }
    }

    /// UserPromptSubmit / PreToolUse / PostToolUse → Claude is (re)starting
    /// work. PostToolUse is what clears WaitingInput after a permission
    /// prompt is accepted: Claude Code emits no hook at the moment of
    /// acceptance, so we wait for the gated tool to finish and treat that
    /// as the "prompt was answered" signal. Yellow lingers for the tool's
    /// runtime — there's no way to do better without an optimistic clear
    /// off the user's keystroke.
    async fn handle_resume_working(&self, msg: HookMessage) {
        self.set_turn_from_hook(&msg, SessionRuntimeState::Working, None)
            .await;
    }

    async fn handle_stop(&self, msg: HookMessage) {
        self.set_turn_from_hook(&msg, SessionRuntimeState::Idle, None)
            .await;
    }

    async fn handle_notify(&self, msg: HookMessage) {
        // auth_success / elicitation_dialog don't represent a turn flip —
        // just log and bail. permission_prompt / idle_prompt both put the
        // session into WaitingInput; the notification_type is carried on
        // so the UI can render permission prompts more urgently.
        let state = match msg.notification_type.as_deref() {
            Some("permission_prompt") | Some("idle_prompt") => {
                SessionRuntimeState::WaitingInput
            }
            other => {
                debug!(
                    notification_type = ?other,
                    "ignoring Notification hook (non-turn event)"
                );
                return;
            }
        };
        let nt = msg.notification_type.clone();
        self.set_turn_from_hook(&msg, state, nt).await;
    }

    /// PermissionRequest fires whenever Claude Code shows a permission
    /// dialog, including sandbox-escape prompts (network / filesystem) that
    /// Notification doesn't cover.
    async fn handle_permission_request(&self, msg: HookMessage) {
        self.set_turn_from_hook(
            &msg,
            SessionRuntimeState::WaitingInput,
            Some("permission_request".to_string()),
        )
        .await;
    }

    /// Elicitation fires when an MCP server requests user input during a
    /// tool call — same turn semantics as a permission prompt.
    async fn handle_elicitation(&self, msg: HookMessage) {
        self.set_turn_from_hook(
            &msg,
            SessionRuntimeState::WaitingInput,
            Some("elicitation".to_string()),
        )
        .await;
    }

    /// Find the Tethys session this hook belongs to and flip its turn state.
    /// Matches first on `claude_session_id` directly. Falls back to the
    /// parent session when the hook comes from a subagent — subagent
    /// transcripts live at `.../<parent-uuid>/subagents/agent-*.jsonl`, so
    /// the parent's claude_session_id is recoverable from `transcript_path`.
    async fn set_turn_from_hook(
        &self,
        msg: &HookMessage,
        state: SessionRuntimeState,
        notification_type: Option<String>,
    ) {
        let Some(csid) = msg.session_id.as_deref() else {
            debug!(
                event = %msg.event,
                transcript_path = ?msg.transcript_path,
                spawn_token = ?msg.spawn_token,
                "hook missing session_id — cannot correlate",
            );
            return;
        };
        let parent_csid = msg
            .transcript_path
            .as_deref()
            .and_then(parent_session_from_subagent_path);
        let lookup = self
            .store
            .read(|s| {
                for ws in &s.workspaces {
                    for sess in &ws.sessions {
                        let tracked = sess.claude_session_id.as_deref();
                        if tracked == Some(csid)
                            || (parent_csid.is_some()
                                && tracked == parent_csid.as_deref())
                        {
                            return Some((ws.id.clone(), sess.id.clone()));
                        }
                    }
                }
                None
            })
            .await;
        let Some((ws_id, sess_id)) = lookup else {
            debug!(
                claude_session_id = csid,
                transcript_path = ?msg.transcript_path,
                "hook for unknown Claude session (not a Tethys-spawned one)"
            );
            return;
        };
        self.set_turn(&sess_id, &ws_id, state, notification_type).await;
    }

    async fn handle_session_start(&self, msg: HookMessage) {
        let Some(token) = msg.spawn_token.as_deref() else {
            debug!("SessionStart without spawn_token — not a Tethys session");
            return;
        };
        let Some(claude_session_id) = msg.session_id.clone() else {
            warn!("SessionStart hook missing session_id");
            return;
        };

        let pending = {
            let mut pending = self.pending.lock().unwrap();
            pending.remove(token)
        };
        let Some(pending) = pending else {
            warn!(token, "SessionStart hook arrived with no matching pending spawn");
            return;
        };

        let transcript_path = msg.transcript_path.as_deref().map(PathBuf::from);
        let workspace_id = pending.workspace_id.clone();
        let session_id = pending.session_id.clone();

        let update = self
            .store
            .mutate(|state| {
                let Some(ws) = state.find_workspace_mut(&workspace_id) else {
                    return Ok(false);
                };
                let Some(session) = ws.sessions.iter_mut().find(|s| s.id == session_id)
                else {
                    return Ok(false);
                };
                session.claude_session_id = Some(claude_session_id.clone());
                session.transcript_path = transcript_path.clone();
                Ok(true)
            })
            .await;

        match update {
            Ok(true) => {
                info!(
                    %session_id,
                    %claude_session_id,
                    source = msg.source.as_deref().unwrap_or("?"),
                    "correlated SessionStart hook",
                );
                let _ = self.app.emit(
                    "workspace:changed",
                    serde_json::json!({ "workspace_id": workspace_id }),
                );
            }
            Ok(false) => warn!(
                %session_id,
                "SessionStart: no matching ClaudeSessionMeta in state"
            ),
            Err(e) => warn!(error = %e, "store mutate during SessionStart failed"),
        }
    }

    /// Register a new output subscriber and return the current scrollback.
    /// The frontend writes the scrollback into xterm first, then drains the
    /// channel for live bytes — zero gap.
    pub fn attach(
        &self,
        session_id: &str,
        channel: Channel<InvokeResponseBody>,
    ) -> AppResult<Vec<u8>> {
        let sessions = self.sessions.lock().unwrap();
        let handle = sessions
            .get(session_id)
            .ok_or_else(|| AppError::Other(format!("session not found: {session_id}")))?;

        let scrollback: Vec<u8> = handle.ring.lock().unwrap().iter().copied().collect();
        handle.subscribers.lock().unwrap().push(channel);
        Ok(scrollback)
    }

    pub fn send_input(&self, session_id: &str, data: &[u8]) -> AppResult<()> {
        let writer = {
            let sessions = self.sessions.lock().unwrap();
            sessions
                .get(session_id)
                .ok_or_else(|| AppError::Other(format!("session not found: {session_id}")))?
                .writer
                .clone()
        };
        // Trace small writes so we can diagnose stray bytes (e.g. the
        // mystery newline-on-first-resume). Normal keystrokes are also
        // small; noisy in logs but useful while we iterate.
        if data.len() <= 16 {
            debug!(
                session_id,
                bytes = %format_bytes(data),
                "send_input"
            );
        }
        writer
            .lock()
            .unwrap()
            .write_all(data)
            .map_err(|e| AppError::Other(format!("write: {e}")))?;
        Ok(())
    }

    pub fn resize(&self, session_id: &str, cols: u16, rows: u16) -> AppResult<()> {
        let sessions = self.sessions.lock().unwrap();
        let handle = sessions
            .get(session_id)
            .ok_or_else(|| AppError::Other(format!("session not found: {session_id}")))?;
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

    pub fn list_for_workspace(&self, workspace_id: &str) -> Vec<SessionInfo> {
        let turn_map = self.turn.lock().unwrap().clone();
        let sessions = self.sessions.lock().unwrap();
        sessions
            .values()
            .filter(|h| h.info.workspace_id == workspace_id)
            .map(|h| {
                let mut info = h.info.clone();
                info.running = *h.running.lock().unwrap();
                let turn = turn_map.get(&h.info.id).cloned().unwrap_or_default();
                info.runtime_state = turn.state;
                info.notification_type = turn.notification_type;
                info.turn_acknowledged = turn.acknowledged;
                info
            })
            .collect()
    }
}

fn new_session_id() -> SessionId {
    Uuid::new_v4().to_string()
}

/// If `transcript_path` looks like a subagent transcript
/// (`.../<parent-uuid>/subagents/agent-*.jsonl`), return the parent uuid so
/// subagent hooks can be routed to the parent session. Returns `None` for
/// parent-level transcripts or any other shape.
fn parent_session_from_subagent_path(transcript_path: &str) -> Option<String> {
    let path = Path::new(transcript_path);
    let file = path.file_name()?.to_str()?;
    if !(file.starts_with("agent-") && file.ends_with(".jsonl")) {
        return None;
    }
    let subagents_dir = path.parent()?;
    if subagents_dir.file_name()?.to_str()? != "subagents" {
        return None;
    }
    Some(subagents_dir.parent()?.file_name()?.to_str()?.to_string())
}

/// Hex + ASCII formatter for tracing bytes sent to the PTY. Control
/// codes show as `\xNN`; printable bytes show as themselves.
fn format_bytes(data: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(data.len() * 4);
    out.push('"');
    for &b in data {
        if (0x20..0x7f).contains(&b) && b != b'\\' && b != b'"' {
            out.push(b as char);
        } else {
            let _ = write!(out, "\\x{b:02x}");
        }
    }
    out.push('"');
    out
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
                    debug!("pty reader: EOF");
                    break;
                }
                Ok(n) => {
                    let chunk = &buf[..n];
                    append_to_ring(&ring, chunk);
                    // Fan out, dropping subscribers whose channel errored.
                    let mut subs = subscribers.lock().unwrap();
                    subs.retain(|sub| {
                        sub.send(InvokeResponseBody::Raw(chunk.to_vec())).is_ok()
                    });
                }
                Err(e) => {
                    warn!(error = %e, "pty reader error");
                    break;
                }
            }
        }
    });
}

/// Scan the tail of the ring for tmux's detach epilogue
/// (`[detached (from session …)]` + surrounding CR/LFs) and remove it.
/// Tmux emits this line to the client's terminal right before the client
/// exits, so it lands in our buffer via the reader thread. Called from
/// the child watcher once we've confirmed the session itself is gone.
fn trim_detach_epilogue(ring: &Arc<Mutex<VecDeque<u8>>>) {
    const NEEDLE: &[u8] = b"[detached ";
    // Search back at most ~256 bytes — the message is short.
    const SCAN_WINDOW: usize = 256;

    let mut ring = ring.lock().unwrap();
    if ring.is_empty() {
        return;
    }
    let tail_start = ring.len().saturating_sub(SCAN_WINDOW);
    // make_contiguous so we can call windows() on a single &[u8] slice.
    let bytes = ring.make_contiguous();
    let Some(rel) = bytes[tail_start..]
        .windows(NEEDLE.len())
        .rposition(|w| w == NEEDLE)
    else {
        return;
    };
    // Truncate from the byte preceding the pattern, walking back over
    // any trailing CR/LF so we don't leave a blank line either.
    let mut cut_from = tail_start + rel;
    while cut_from > 0 && matches!(bytes[cut_from - 1], b'\r' | b'\n') {
        cut_from -= 1;
    }
    ring.truncate(cut_from);
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

fn spawn_child_watcher(
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
    session_id: SessionId,
    workspace_id: String,
    running: Arc<Mutex<bool>>,
    ring: Arc<Mutex<VecDeque<u8>>>,
    tmux_bin: PathBuf,
    app: AppHandle,
) {
    std::thread::spawn(move || {
        let status = child.wait();
        *running.lock().unwrap() = false;
        let code = status.ok().map(|s| s.exit_code() as i32);

        // The child here is the tmux *client*. It exits both when claude
        // truly ends (session disappears) and when the client merely
        // detaches (app shutdown, another client steals with -D, etc.).
        // Check has-session to tell them apart.
        if tmux::has_session(&tmux_bin, &session_id) {
            info!(
                %session_id,
                ?code,
                "tmux client exited but session still alive (detach)"
            );
            return;
        }

        // Session truly gone — tmux client printed `[detached (from
        // session …)]` to the pty just before exiting. Strip that
        // trailing line from the ring so it doesn't surface when the
        // user revisits the workspace.
        trim_detach_epilogue(&ring);

        info!(%session_id, ?code, "session child exited");
        let _ = app.emit(
            "session:exit",
            serde_json::json!({
                "workspace_id": workspace_id,
                "session_id": session_id,
                "code": code,
            }),
        );
        // Turn state is stale once the PTY is gone — emit a Dormant
        // transition so the UI doesn't keep showing Working indefinitely.
        let _ = app.emit(
            "session:turn_changed",
            serde_json::json!({
                "workspace_id": workspace_id,
                "session_id": session_id,
                "runtime_state": SessionRuntimeState::Dormant,
                "notification_type": Option::<String>::None,
            }),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::parent_session_from_subagent_path;

    #[test]
    fn extracts_parent_uuid_from_subagent_transcript() {
        let parent = "0bd83a02-04d6-4139-b007-388eea214e22";
        let path = format!(
            "/Users/ryan/.claude/projects/-Users-ryan-code-worktrees-foo/{parent}/subagents/agent-a9cc54ae168591b32.jsonl"
        );
        assert_eq!(
            parent_session_from_subagent_path(&path).as_deref(),
            Some(parent)
        );
    }

    #[test]
    fn returns_none_for_parent_level_transcript() {
        let parent = "0bd83a02-04d6-4139-b007-388eea214e22";
        let path = format!(
            "/Users/ryan/.claude/projects/-Users-ryan-code-worktrees-foo/{parent}.jsonl"
        );
        assert_eq!(parent_session_from_subagent_path(&path), None);
    }

    #[test]
    fn returns_none_for_unrelated_path() {
        assert_eq!(parent_session_from_subagent_path("/tmp/foo.jsonl"), None);
        assert_eq!(parent_session_from_subagent_path(""), None);
    }
}

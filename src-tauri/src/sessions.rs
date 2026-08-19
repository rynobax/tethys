use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::hook_listener::HookMessage;
use crate::pty::{OnExit, PtyProcess, PtySpawn, Ring};
use crate::state::SessionRuntimeState;
use crate::store::Store;
use crate::tmux;

const RING_CAPACITY: usize = 2 * 1024 * 1024; // 2 MB scrollback per session

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
    /// Runtime state to seed the session with before any hooks arrive. A
    /// brand-new spawn sits at an empty prompt (`WaitingInput`); a reattach
    /// may be mid-response (`Working`).
    initial_state: SessionRuntimeState,
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
    pty: PtyProcess,
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

impl TurnState {
    /// Apply an incoming turn signal. Returns `true` when the UI needs a
    /// fresh `session:turn_changed` (state/notification changed, or a
    /// dismissed indicator must re-light because a new signal arrived for
    /// the same state). A repeated signal for an already-lit state is a
    /// no-op so we don't spam redundant events.
    fn apply(
        &mut self,
        state: SessionRuntimeState,
        notification_type: Option<String>,
    ) -> bool {
        let unchanged =
            self.state == state && self.notification_type == notification_type;
        // An acknowledged indicator re-lights on the next signal even when the
        // state is identical: a repeated idle_prompt is a fresh nudge, not
        // noise, so a cleared "your turn" row doesn't stay dark forever.
        if unchanged && !self.acknowledged {
            return false;
        }
        self.state = state;
        self.notification_type = notification_type;
        self.acknowledged = false;
        true
    }
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
            current.apply(state, notification_type.clone())
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
            .update_workspace_quiet(workspace_id, |ws| {
                if let Some(meta) = ws.session_mut(session_id) {
                    meta.runtime_state = Some(state);
                    meta.notification_type = notification_type.clone();
                    meta.turn_acknowledged = false;
                }
                Ok(())
            })
            .await;
        if let Err(e) = persist {
            warn!(error = %e, session_id, "persist turn state failed");
        }
    }

    /// Reconcile hook-derived turn state against Claude's own status probe
    /// files. For each probe correlated to a running Tethys session
    /// (`sessionId` == `claude_session_id`), compare the probe-derived state
    /// to what we're currently showing; on a mismatch, log it (the reason
    /// this exists — instrumentation to judge whether probes beat hooks) and
    /// apply the probe state as authoritative. The probe survives subagent
    /// activity and dropped hooks, so it corrects drift the hook stream
    /// can't see. Dormant sessions are left alone — a dead PTY is the exit
    /// hook's job, and a lingering probe file must not resurrect one.
    pub async fn reconcile_probes(&self, probes: &[crate::probe::Probe]) {
        let parsed: Vec<ProbeView> = probes
            .iter()
            .filter_map(|p| {
                let sid = p.session_id.as_deref()?;
                let state = crate::probe::state_from_status(p.status.as_deref()?)?;
                Some(ProbeView {
                    sid,
                    cwd: p.cwd.as_deref(),
                    state,
                    status_updated_at: p.status_updated_at,
                })
            })
            .collect();
        if parsed.is_empty() {
            return;
        }

        // Only running sessions are eligible; a lingering probe file must not
        // resurrect a dead one, and Dormant is the exit hook's job.
        let running: HashSet<SessionId> = {
            let sessions = self.sessions.lock().unwrap();
            sessions
                .iter()
                .filter(|(_, h)| h.pty.is_running())
                .map(|(id, _)| id.clone())
                .collect()
        };

        let actions = self
            .store
            .read(|s| {
                let running = &running;
                let tracked: Vec<TrackedSession> = s
                    .workspaces
                    .iter()
                    .flat_map(|ws| {
                        ws.sessions.iter().map(move |se| TrackedSession {
                            workspace_id: ws.id.as_str(),
                            session_id: &se.id,
                            cwd: se.cwd.to_str(),
                            claude_session_id: se.claude_session_id.as_deref(),
                            running: running.contains(&se.id),
                        })
                    })
                    .collect();
                plan_probe_reconciliation(&parsed, &tracked)
            })
            .await;

        for ProbeAction { workspace_id: ws_id, session_id: sess_id, state: probe_state, heal_to } in
            actions
        {
            if let Some(new_csid) = heal_to {
                warn!(
                    session_id = %sess_id,
                    %new_csid,
                    "healed stale claude_session_id — Claude rotated its session id (compaction/resume)"
                );
                let ws = ws_id.clone();
                let sid = sess_id.clone();
                let csid = new_csid.clone();
                let healed = self
                    .store
                    .update_workspace_quiet(&ws, move |ws| {
                        if let Some(m) = ws.session_mut(&sid) {
                            m.claude_session_id = Some(csid);
                        }
                        Ok(())
                    })
                    .await;
                if let Err(e) = healed {
                    warn!(error = %e, session_id = %sess_id, "persist healed session id failed");
                }
            }

            let (current, current_nt) = {
                let map = self.turn.lock().unwrap();
                match map.get(&sess_id) {
                    Some(t) => (t.state, t.notification_type.clone()),
                    None => (SessionRuntimeState::default(), None),
                }
            };
            if current == probe_state {
                continue;
            }

            warn!(
                session_id = %sess_id,
                hook_state = ?current,
                probe_state = ?probe_state,
                "probe/hook turn-state mismatch — applying probe (authoritative)"
            );
            // The probe can't distinguish permission_prompt from idle_prompt,
            // so keep the hook's subtype when we're still waiting on input.
            let nt = if probe_state == SessionRuntimeState::WaitingInput {
                current_nt
            } else {
                None
            };
            self.set_turn(&sess_id, &ws_id, probe_state, nt).await;
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
            .update_workspace_quiet(workspace_id, |ws| {
                if let Some(meta) = ws.session_mut(session_id) {
                    meta.turn_acknowledged = true;
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
            initial_state,
        } = req;
        let info = SessionInfo {
            id: id.clone(),
            workspace_id: workspace_id.clone(),
            repo_key,
            cwd: cwd.to_path_buf(),
            running: true,
            runtime_state: initial_state,
            notification_type: None,
            turn_acknowledged: false,
        };

        let pty = PtyProcess::spawn(
            PtySpawn {
                program,
                args,
                cwd,
                seed_bytes,
                ring_capacity: RING_CAPACITY,
                tmux_session_name: id.clone(),
                tmux_bin,
            },
            session_exit_hook(self.app.clone(), workspace_id.clone(), id.clone()),
        )?;

        let handle = SessionHandle {
            info: info.clone(),
            pty,
        };

        self.sessions.lock().unwrap().insert(id.clone(), handle);
        // Seed the caller-provided initial state. Hooks refine it shortly: a
        // brand-new spawn waits at the prompt (WaitingInput), a reattach is
        // assumed mid-response (Working).
        self.turn.lock().unwrap().insert(
            id,
            TurnState {
                state: initial_state,
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
            // Freshly-spawned (or freshly-resumed) Claude lands at an empty
            // prompt waiting on the user — show "your turn", not "working".
            initial_state: SessionRuntimeState::WaitingInput,
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
            // A reattached session may be mid-response; assume Working and let
            // hooks correct it.
            initial_state: SessionRuntimeState::Working,
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
            .update_workspace(&workspace_id, |ws| {
                let Some(session) = ws.session_mut(&session_id) else {
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
        Ok(handle.pty.attach(channel))
    }

    /// Best-effort: drop a subscriber (by its channel id) when its pane
    /// unmounts. Silently ignores an unknown session — it may already be gone,
    /// and the only goal is to stop streaming to a dead terminal.
    pub fn detach(&self, session_id: &str, channel_id: u32) {
        if let Some(handle) = self.sessions.lock().unwrap().get(session_id) {
            handle.pty.detach(channel_id);
        }
    }

    pub fn send_input(&self, session_id: &str, data: &[u8]) -> AppResult<()> {
        let sessions = self.sessions.lock().unwrap();
        sessions
            .get(session_id)
            .ok_or_else(|| AppError::Other(format!("session not found: {session_id}")))?
            .pty
            .send_input(data)
    }

    pub fn resize(&self, session_id: &str, cols: u16, rows: u16) -> AppResult<()> {
        let sessions = self.sessions.lock().unwrap();
        sessions
            .get(session_id)
            .ok_or_else(|| AppError::Other(format!("session not found: {session_id}")))?
            .pty
            .resize(cols, rows)
    }

    pub fn list_for_workspace(&self, workspace_id: &str) -> Vec<SessionInfo> {
        let turn_map = self.turn.lock().unwrap().clone();
        let sessions = self.sessions.lock().unwrap();
        sessions
            .values()
            .filter(|h| h.info.workspace_id == workspace_id)
            .map(|h| {
                let mut info = h.info.clone();
                info.running = h.pty.is_running();
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

/// Build the exit hook handed to [`PtyProcess::spawn`]. It runs only on a
/// true child exit (the watcher already filtered out client detaches): scrub
/// tmux's detach epilogue from the ring, then emit `session:exit` and a
/// `Dormant` `session:turn_changed` so the UI stops showing Working forever.
fn session_exit_hook(app: AppHandle, workspace_id: String, session_id: SessionId) -> OnExit {
    Box::new(move |code, ring| {
        // Session truly gone — tmux client printed `[detached (from
        // session …)]` to the pty just before exiting. Strip that trailing
        // line from the ring so it doesn't surface when the user revisits
        // the workspace.
        trim_detach_epilogue(ring);

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
    })
}

/// Scan the tail of the ring for tmux's detach epilogue
/// (`[detached (from session …)]` + surrounding CR/LFs) and remove it.
/// Tmux emits this line to the client's terminal right before the client
/// exits, so it lands in our buffer via the reader thread. Called from
/// the exit hook once we've confirmed the session itself is gone.
fn trim_detach_epilogue(ring: &Ring) {
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

/// A probe reduced to the fields the reconciler correlates on.
struct ProbeView<'a> {
    sid: &'a str,
    cwd: Option<&'a str>,
    state: SessionRuntimeState,
    status_updated_at: Option<i64>,
}

/// A tracked Tethys session a probe can be correlated to.
struct TrackedSession<'a> {
    workspace_id: &'a str,
    session_id: &'a SessionId,
    cwd: Option<&'a str>,
    claude_session_id: Option<&'a str>,
    running: bool,
}

/// One reconciliation decision: apply `state` to `session_id`, first
/// rewriting its stored `claude_session_id` when `heal_to` is set.
#[derive(Debug, PartialEq)]
struct ProbeAction {
    workspace_id: String,
    session_id: SessionId,
    state: SessionRuntimeState,
    heal_to: Option<String>,
}

/// Keep, per cwd, only the probe with the newest `status_updated_at`.
///
/// Claude leaves a session's old probe file behind when it rotates its
/// session id (compaction/resume), so one cwd can show several probes at
/// once — but Tethys runs a single live Claude per worktree cwd, so only the
/// freshest is current. Dropping the stale ones up front stops a ghost probe
/// from winning primary correlation *and* from keeping a rotated-away id in
/// `live_sids`, which would otherwise suppress the very heal meant to repair
/// it. A missing timestamp sorts oldest so a stamped live probe always wins;
/// probes without a cwd can't be deduped this way and pass through untouched.
fn freshest_probe_per_cwd<'a>(probes: &'a [ProbeView<'a>]) -> Vec<&'a ProbeView<'a>> {
    let mut freshest: HashMap<&str, &ProbeView> = HashMap::new();
    let mut out: Vec<&ProbeView> = Vec::new();
    for p in probes {
        let Some(cwd) = p.cwd else {
            out.push(p);
            continue;
        };
        match freshest.get(cwd) {
            Some(cur)
                if cur.status_updated_at.unwrap_or(i64::MIN)
                    >= p.status_updated_at.unwrap_or(i64::MIN) => {}
            _ => {
                freshest.insert(cwd, p);
            }
        }
    }
    out.extend(freshest.into_values());
    out
}

/// Pure correlation core of `reconcile_probes`, extracted so the drift/heal
/// decisions are unit-testable without a live supervisor. Correlates each
/// probe to a running tracked session: first by session id, then — when that
/// misses because Claude rotated the id (compaction/resume) — by a *single*
/// running session in the same cwd whose stored id has gone stale, carrying
/// the fresh id in `heal_to` so hook correlation is repaired too.
fn plan_probe_reconciliation(
    probes: &[ProbeView],
    sessions: &[TrackedSession],
) -> Vec<ProbeAction> {
    let probes = freshest_probe_per_cwd(probes);
    // Every session id Claude currently reports. A stored `claude_session_id`
    // absent from this set has rotated away — the trigger for cwd healing.
    let live_sids: HashSet<&str> = probes.iter().map(|p| p.sid).collect();
    let mut out = Vec::new();
    for p in probes {
        if let Some(s) =
            sessions.iter().find(|s| s.claude_session_id == Some(p.sid))
        {
            if s.running {
                out.push(ProbeAction {
                    workspace_id: s.workspace_id.to_string(),
                    session_id: s.session_id.clone(),
                    state: p.state,
                    heal_to: None,
                });
            }
            continue;
        }
        let Some(cwd) = p.cwd else { continue };
        let mut stale_in_cwd = sessions.iter().filter(|s| {
            s.running
                && s.cwd == Some(cwd)
                && s.claude_session_id.is_none_or(|c| !live_sids.contains(c))
        });
        if let Some(s) = stale_in_cwd.next() {
            if stale_in_cwd.next().is_none() {
                out.push(ProbeAction {
                    workspace_id: s.workspace_id.to_string(),
                    session_id: s.session_id.clone(),
                    state: p.state,
                    heal_to: Some(p.sid.to_string()),
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::parent_session_from_subagent_path;
    use super::{
        plan_probe_reconciliation, ProbeAction, ProbeView, TrackedSession, TurnState,
    };
    use crate::state::SessionRuntimeState;

    /// Claude rotated its session id and left the old probe file behind, so
    /// the cwd shows two probes: a stale one still bearing the id Tethys
    /// stored, and the fresh live one. The reconciler must ignore the ghost,
    /// heal the stored id to the live one, and apply the live probe's state —
    /// not freeze on the stale probe forever.
    #[test]
    fn stale_ghost_probe_does_not_block_healing() {
        let stored_id = "b6a26662".to_string();
        let sess_id = "tethys-sess".to_string();
        let ws_id = "ws".to_string();
        let cwd = "/wt/custom-fill-in-field/nl-ai";
        let probes = [
            // Ghost: old id (== stored), stale timestamp, still "working".
            ProbeView {
                sid: "b6a26662",
                cwd: Some(cwd),
                state: SessionRuntimeState::Working,
                status_updated_at: Some(1_784_158_540_640),
            },
            // Live: rotated id, fresh timestamp, now idle.
            ProbeView {
                sid: "14a3fff4",
                cwd: Some(cwd),
                state: SessionRuntimeState::Idle,
                status_updated_at: Some(1_784_573_092_544),
            },
        ];
        let sessions = [TrackedSession {
            workspace_id: &ws_id,
            session_id: &sess_id,
            cwd: Some(cwd),
            claude_session_id: Some(&stored_id),
            running: true,
        }];

        let actions = plan_probe_reconciliation(&probes, &sessions);

        assert_eq!(
            actions,
            vec![ProbeAction {
                workspace_id: ws_id,
                session_id: sess_id,
                state: SessionRuntimeState::Idle,
                heal_to: Some("14a3fff4".to_string()),
            }]
        );
    }

    /// A single live probe whose id already matches the stored id needs no
    /// healing — apply its state straight through.
    #[test]
    fn matching_probe_applies_state_without_healing() {
        let stored_id = "sid-1".to_string();
        let sess_id = "sess".to_string();
        let ws_id = "ws".to_string();
        let probes = [ProbeView {
            sid: "sid-1",
            cwd: Some("/wt/a"),
            state: SessionRuntimeState::WaitingInput,
            status_updated_at: Some(10),
        }];
        let sessions = [TrackedSession {
            workspace_id: &ws_id,
            session_id: &sess_id,
            cwd: Some("/wt/a"),
            claude_session_id: Some(&stored_id),
            running: true,
        }];
        let actions = plan_probe_reconciliation(&probes, &sessions);
        assert_eq!(
            actions,
            vec![ProbeAction {
                workspace_id: ws_id,
                session_id: sess_id,
                state: SessionRuntimeState::WaitingInput,
                heal_to: None,
            }]
        );
    }

    /// A probe for a non-running session must never produce an action — a
    /// lingering probe file can't resurrect a dead PTY.
    #[test]
    fn probe_never_resurrects_a_dead_session() {
        let stored_id = "sid-1".to_string();
        let sess_id = "sess".to_string();
        let ws_id = "ws".to_string();
        let probes = [ProbeView {
            sid: "sid-1",
            cwd: Some("/wt/a"),
            state: SessionRuntimeState::Working,
            status_updated_at: Some(10),
        }];
        let sessions = [TrackedSession {
            workspace_id: &ws_id,
            session_id: &sess_id,
            cwd: Some("/wt/a"),
            claude_session_id: Some(&stored_id),
            running: false,
        }];
        assert!(plan_probe_reconciliation(&probes, &sessions).is_empty());
    }

    #[test]
    fn apply_reports_change_on_state_transition() {
        let mut turn = TurnState::default();
        assert!(turn.apply(SessionRuntimeState::Working, None));
        assert_eq!(turn.state, SessionRuntimeState::Working);
        assert!(!turn.acknowledged);
    }

    #[test]
    fn apply_is_noop_for_repeated_unacknowledged_signal() {
        let mut turn = TurnState::default();
        turn.apply(
            SessionRuntimeState::WaitingInput,
            Some("idle_prompt".into()),
        );
        // Same state + notification, still lit — nothing to re-emit.
        assert!(!turn.apply(
            SessionRuntimeState::WaitingInput,
            Some("idle_prompt".into())
        ));
    }

    #[test]
    fn apply_relights_acknowledged_indicator_on_repeated_signal() {
        let mut turn = TurnState::default();
        turn.apply(
            SessionRuntimeState::WaitingInput,
            Some("idle_prompt".into()),
        );
        turn.acknowledged = true; // user cleared the notification

        // A repeated idle_prompt for the same state must re-light the row.
        assert!(turn.apply(
            SessionRuntimeState::WaitingInput,
            Some("idle_prompt".into())
        ));
        assert!(!turn.acknowledged);
    }

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

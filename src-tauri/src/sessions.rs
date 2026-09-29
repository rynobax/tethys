use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::Serialize;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::artifacts::{self, ArtifactStore};
use crate::error::{AppError, AppResult};
use crate::agent::Agent;
use crate::paths::Paths;
use crate::agent_bin::AgentBins;
use crate::agent_cmd;
use crate::hook_listener::HookMessage;
use crate::mcp::McpLaunch;
use crate::pty::{OnExit, PtyProcess, PtySpawn, Ring};
use crate::state::{AgentSessionMeta, SessionRuntimeState, WorkspaceId};
use crate::store::Store;
use crate::turn::{TurnChanged, TurnSignal, TurnState, TurnTracker};
use crate::tmux;

const RING_CAPACITY: usize = 2 * 1024 * 1024;

pub struct SpawnAgent<'a> {
    pub agent: Agent,
    pub workspace_id: String,
    pub cwd: &'a Path,
    pub tmux_bin: &'a Path,
    pub agent_bin: &'a Path,
    pub extra_writable: &'a [PathBuf],
    pub resume_session_id: Option<&'a str>,
    pub mcp: Option<&'a McpLaunch>,
    pub brief: Option<&'a str>,
}

struct SpawnRequest<'a> {
    id: SessionId,
    workspace_id: String,
    cwd: &'a Path,
    program: &'a Path,
    args: &'a [String],
    tmux_bin: PathBuf,
    seed_bytes: &'a [u8],
    seed: TurnSignal,
}

pub type SessionId = String;

#[derive(Debug, Clone, Serialize)]
pub struct SessionInfo {
    pub id: SessionId,
    pub workspace_id: String,
    pub cwd: PathBuf,
    pub running: bool,
    pub runtime_state: SessionRuntimeState,
    pub notification_type: Option<String>,
    pub turn_acknowledged: bool,
    /// Derived here so every view of a session agrees on it.
    pub needs_turn: bool,
    pub working: bool,
    pub tui_ready: bool,
}

struct SessionHandle {
    info: SessionInfo,
    pty: PtyProcess,
}

struct PendingSpawn {
    workspace_id: String,
    session_id: SessionId,
    expires_at: Instant,
}

const PENDING_TTL: Duration = Duration::from_secs(30);

pub struct SessionSupervisor {
    sessions: Mutex<HashMap<SessionId, SessionHandle>>,
    /// Keyed by `TETHYS_SPAWN_TOKEN`, until SessionStart reports the
    /// agent's session id.
    pending: Mutex<HashMap<String, PendingSpawn>>,
    /// Kept off the store because every keystroke lands here; `idle` drains
    /// it into `last_active_at` on each sweep.
    activity: Mutex<HashMap<SessionId, DateTime<Utc>>>,
    turn: Arc<TurnTracker>,
    artifacts: Arc<ArtifactStore>,
    store: Arc<Store>,
    app: AppHandle,
}

impl SessionSupervisor {
    pub fn new(app: AppHandle, store: Arc<Store>, artifacts: Arc<ArtifactStore>) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            activity: Mutex::new(HashMap::new()),
            turn: Arc::new(TurnTracker::new()),
            artifacts,
            store,
            app,
        }
    }

    /// Must run after `reattach_tmux`, whose `Working` seed is a guess the
    /// persisted state beats.
    pub fn restore_turn(
        &self,
        session_id: &str,
        state: SessionRuntimeState,
        notification_type: Option<String>,
        acknowledged: bool,
    ) {
        self.turn.observe(
            session_id,
            "",
            TurnSignal::Restored {
                state,
                notification_type,
                acknowledged,
            },
        );
    }

    /// Every turn-state source goes through here, so disagreements between
    /// them are settled by `TurnTracker::observe`, not by call order.
    async fn apply_signal(
        &self,
        session_id: &str,
        workspace_id: &str,
        signal: TurnSignal,
    ) {
        let Some(changed) = self.turn.observe(session_id, workspace_id, signal) else {
            return;
        };
        let running = self
            .sessions
            .lock()
            .unwrap()
            .get(session_id)
            .is_some_and(|h| h.pty.is_running());
        publish_turn(&self.app, &changed, running);
        if let Err(e) = persist_turn(&self.store, &changed).await {
            warn!(error = %e, session_id, "persist turn state failed");
        }
    }

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

        // A lingering probe file must not resurrect a dead session.
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
                    // Else a dead Claude probe could heal onto a codex session
                    // in the same cwd.
                    .filter(|ws| ws.agent == Agent::Claude)
                    .flat_map(|ws| {
                        ws.session.iter().map(move |se| TrackedSession {
                            workspace_id: ws.id.as_str(),
                            session_id: &se.id,
                            cwd: se.cwd.to_str(),
                            agent_session_id: se.agent_session_id.as_deref(),
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
                    "healed stale agent_session_id — Claude rotated its session id (compaction/resume)"
                );
                let ws = ws_id.clone();
                let sid = sess_id.clone();
                let csid = new_csid.clone();
                let healed = self
                    .store
                    .update_workspace_quiet(&ws, move |ws| {
                        if let Some(m) = ws.session_mut(&sid) {
                            m.agent_session_id = Some(csid);
                        }
                        Ok(())
                    })
                    .await;
                if let Err(e) = healed {
                    warn!(error = %e, session_id = %sess_id, "persist healed session id failed");
                }
            }

            let before = self.turn.get(&sess_id).state;
            self.apply_signal(
                &sess_id,
                &ws_id,
                TurnSignal::Probe { state: probe_state },
            )
            .await;
            let after = self.turn.get(&sess_id).state;
            if before != after {
                warn!(
                    session_id = %sess_id,
                    hook_state = ?before,
                    probe_state = ?probe_state,
                    "probe/hook turn-state mismatch — applied probe (authoritative)"
                );
            }
        }
    }

    pub async fn acknowledge_turn(&self, session_id: &str, workspace_id: &str) {
        self.apply_signal(session_id, workspace_id, TurnSignal::Acknowledged)
            .await;
    }

    /// `id` doubles as the tmux session name.
    fn spawn_with_id(&self, req: SpawnRequest<'_>) -> AppResult<SessionInfo> {
        let SpawnRequest {
            id,
            workspace_id,
            cwd,
            program,
            args,
            tmux_bin,
            seed_bytes,
            seed,
        } = req;
        let seed_state = match seed {
            TurnSignal::Reattached => SessionRuntimeState::Working,
            _ => SessionRuntimeState::WaitingInput,
        };
        let info = SessionInfo {
            id: id.clone(),
            workspace_id: workspace_id.clone(),
            cwd: cwd.to_path_buf(),
            running: true,
            runtime_state: seed_state,
            notification_type: None,
            turn_acknowledged: false,
            needs_turn: false,
            working: false,
            tui_ready: false,
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
            session_ready_hook(self.app.clone(), workspace_id.clone()),
            session_exit_hook(
                self.app.clone(),
                self.store.clone(),
                self.turn.clone(),
                workspace_id.clone(),
                id.clone(),
            ),
        )?;

        let handle = SessionHandle {
            info: info.clone(),
            pty,
        };

        self.sessions.lock().unwrap().insert(id.clone(), handle);
        self.turn.observe(&id, &workspace_id, seed);
        let _ = self.app.emit(
            "session:changed",
            serde_json::json!({ "workspace_id": workspace_id }),
        );
        Ok(info)
    }

    pub fn spawn_agent(&self, req: SpawnAgent<'_>) -> AppResult<(SessionInfo, String)> {
        let token = Uuid::new_v4().to_string();
        let id = new_session_id();

        let hook_bin = crate::paths::tethys_hook_bin()
            .ok()
            .filter(|p| p.exists());
        let command = agent_cmd::build(agent_cmd::Spawn {
            agent: req.agent,
            agent_bin: req.agent_bin,
            workspace_id: &req.workspace_id,
            session_id: &id,
            spawn_token: &token,
            hook_bin: hook_bin.as_deref(),
            extra_writable: req.extra_writable,
            resume_session_id: req.resume_session_id,
            mcp: req.mcp,
            brief: req.brief,
        })?;

        let args = tmux::new_session_args(
            &id,
            &[("TETHYS_SPAWN_TOKEN", token.clone())],
            &command,
        );

        let workspace_id = req.workspace_id;
        let tmux_bin = req.tmux_bin;
        let info = self.spawn_with_id(SpawnRequest {
            id,
            workspace_id: workspace_id.clone(),
            cwd: req.cwd,
            program: tmux_bin,
            args: &args,
            tmux_bin: tmux_bin.to_path_buf(),
            seed_bytes: &[],
            seed: TurnSignal::Spawned,
        })?;

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

    pub fn reattach_tmux(
        &self,
        session_id: SessionId,
        workspace_id: String,
        cwd: &Path,
        tmux_bin: &Path,
    ) -> AppResult<SessionInfo> {
        if !tmux::has_session(tmux_bin, &session_id) {
            return Err(AppError::Other(format!(
                "tmux session {session_id} no longer exists"
            )));
        }
        // Before attaching: the attach repaints only the visible area.
        let seed = tmux::capture_pane(tmux_bin, &session_id).unwrap_or_default();

        let args = tmux::attach_session_args(&session_id);
        self.spawn_with_id(SpawnRequest {
            id: session_id,
            workspace_id,
            cwd,
            program: tmux_bin,
            args: &args,
            tmux_bin: tmux_bin.to_path_buf(),
            seed_bytes: &seed,
            seed: TurnSignal::Reattached,
        })
    }

    pub async fn handle_hook_event(&self, msg: HookMessage) {
        match msg.event.as_str() {
            "session-start" => self.handle_session_start(msg).await,
            "user-submit" | "pre-tool" => self.handle_resume_working(msg).await,
            "post-tool" => {
                self.record_page_if_written(&msg).await;
                self.handle_resume_working(msg).await
            }
            // `interrupt` is codex's escape.
            "stop" | "stop-failure" | "interrupt" => self.handle_stop(msg).await,
            "notify" => self.handle_notify(msg).await,
            "permission-request" => self.handle_permission_request(msg).await,
            "elicitation" => self.handle_elicitation(msg).await,
            other => debug!(event = %other, "unknown hook event"),
        }
    }

    /// No hook fires when a permission prompt is accepted, so PostToolUse of
    /// the gated tool is what clears `WaitingInput`.
    async fn handle_resume_working(&self, msg: HookMessage) {
        self.set_turn_from_hook(&msg, SessionRuntimeState::Working, None)
            .await;
    }

    async fn handle_stop(&self, msg: HookMessage) {
        self.set_turn_from_hook(&msg, SessionRuntimeState::Idle, None)
            .await;
        let Some(message) = msg.last_assistant_message.as_deref() else { return };
        let Some((ws_id, _)) = self.resolve_session(&msg).await else { return };
        self.artifacts.record_diagrams(&ws_id, message).await;
    }

    async fn record_page_if_written(&self, msg: &HookMessage) {
        let call = artifacts::ToolCall {
            tool_name: msg.tool_name.as_deref(),
            file_path: msg.tool_file_path.as_deref(),
            command: msg.tool_command.as_deref(),
            cwd: msg.cwd.as_deref(),
        };
        let Some(path) = artifacts::page_written(&call) else { return };
        let Some((ws_id, _)) = self.resolve_session(msg).await else { return };
        let root = self
            .store
            .read(|s| s.find_workspace(&ws_id).and_then(|w| w.root_buf()))
            .await;
        let Some(root) = root else { return };
        self.artifacts.record_page(&ws_id, &root, &path).await;
    }

    async fn handle_notify(&self, msg: HookMessage) {
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

    /// Covers sandbox-escape prompts that Notification doesn't.
    async fn handle_permission_request(&self, msg: HookMessage) {
        self.set_turn_from_hook(
            &msg,
            SessionRuntimeState::WaitingInput,
            Some("permission_request".to_string()),
        )
        .await;
    }

    async fn handle_elicitation(&self, msg: HookMessage) {
        self.set_turn_from_hook(
            &msg,
            SessionRuntimeState::WaitingInput,
            Some("elicitation".to_string()),
        )
        .await;
    }

    async fn set_turn_from_hook(
        &self,
        msg: &HookMessage,
        state: SessionRuntimeState,
        notification_type: Option<String>,
    ) {
        let Some((ws_id, sess_id)) = self.resolve_session(msg).await else { return };
        self.touch(&sess_id);
        self.apply_signal(
            &sess_id,
            &ws_id,
            TurnSignal::Hook {
                state,
                notification_type,
            },
        )
        .await;
    }

    /// By `agent_session_id`, then a subagent's parent id, then `cwd`, which
    /// survives the id rotation on compaction/resume.
    async fn resolve_session(&self, msg: &HookMessage) -> Option<(WorkspaceId, SessionId)> {
        let Some(csid) = msg.session_id.as_deref() else {
            debug!(
                event = %msg.event,
                transcript_path = ?msg.transcript_path,
                spawn_token = ?msg.spawn_token,
                "hook missing session_id — cannot correlate",
            );
            return None;
        };
        let parent_csid = msg
            .transcript_path
            .as_deref()
            .and_then(parent_session_from_subagent_path);
        let cwd = msg.cwd.as_deref();
        let lookup = self
            .store
            .read(|s| {
                let mut by_cwd = None;
                for ws in &s.workspaces {
                    let Some(sess) = &ws.session else { continue };
                    let tracked = sess.agent_session_id.as_deref();
                    if tracked == Some(csid)
                        || (parent_csid.is_some() && tracked == parent_csid.as_deref())
                    {
                        return Some((ws.id.clone(), sess.id.clone()));
                    }
                    // An id match elsewhere still wins.
                    if by_cwd.is_none() && cwd.is_some() && sess.cwd.to_str() == cwd {
                        by_cwd = Some((ws.id.clone(), sess.id.clone()));
                    }
                }
                by_cwd
            })
            .await;
        if lookup.is_none() {
            debug!(
                agent_session_id = csid,
                transcript_path = ?msg.transcript_path,
                "hook for unknown Claude session (not a Tethys-spawned one)"
            );
        }
        lookup
    }

    async fn handle_session_start(&self, msg: HookMessage) {
        let Some(token) = msg.spawn_token.as_deref() else {
            debug!("SessionStart without spawn_token — not a Tethys session");
            return;
        };
        let Some(agent_session_id) = msg.session_id.clone() else {
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
                session.agent_session_id = Some(agent_session_id.clone());
                session.transcript_path = transcript_path.clone();
                Ok(true)
            })
            .await;

        match update {
            Ok(true) => {
                info!(
                    %session_id,
                    %agent_session_id,
                    source = msg.source.as_deref().unwrap_or("?"),
                    "correlated SessionStart hook",
                );
            }
            Ok(false) => warn!(
                %session_id,
                "SessionStart: no matching AgentSessionMeta in state"
            ),
            Err(e) => warn!(error = %e, "store mutate during SessionStart failed"),
        }
    }

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

    pub fn detach(&self, session_id: &str, channel_id: u32) {
        if let Some(handle) = self.sessions.lock().unwrap().get(session_id) {
            handle.pty.detach(channel_id);
        }
    }

    pub fn send_input(&self, session_id: &str, data: &[u8]) -> AppResult<()> {
        self.touch(session_id);
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

    /// `None` means dormant: nothing spawned or reattached this run.
    pub fn info(&self, session_id: &str) -> Option<SessionInfo> {
        let sessions = self.sessions.lock().unwrap();
        let h = sessions.get(session_id)?;
        let mut info = h.info.clone();
        info.running = h.pty.is_running();
        let turn = self.turn.get(&h.info.id);
        info.needs_turn = turn.needs_turn(info.running);
        info.working = turn.is_working(info.running);
        info.runtime_state = turn.state;
        info.notification_type = turn.notification_type;
        info.turn_acknowledged = turn.acknowledged;
        info.tui_ready = h.pty.tui_ready();
        Some(info)
    }

    fn touch(&self, session_id: &str) {
        self.activity
            .lock()
            .unwrap()
            .insert(session_id.to_string(), Utc::now());
    }

    pub fn take_activity(&self) -> HashMap<SessionId, DateTime<Utc>> {
        std::mem::take(&mut self.activity.lock().unwrap())
    }

    pub fn forget(&self, session_id: &str) {
        self.sessions.lock().unwrap().remove(session_id);
    }
}

fn new_session_id() -> SessionId {
    Uuid::new_v4().to_string()
}

/// `.../<parent-uuid>/subagents/agent-*.jsonl` → `<parent-uuid>`.
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

fn publish_turn(app: &AppHandle, changed: &TurnChanged, running: bool) {
    let snapshot = TurnState {
        state: changed.runtime_state,
        notification_type: changed.notification_type.clone(),
        acknowledged: changed.turn_acknowledged,
    };
    let _ = app.emit(
        "session:turn_changed",
        TurnChangedEvent {
            changed,
            running,
            needs_turn: snapshot.needs_turn(running),
            working: snapshot.is_working(running),
        },
    );
}

/// Adds the liveness-dependent predicates the pure tracker can't compute.
#[derive(Clone, Serialize)]
struct TurnChangedEvent<'a> {
    #[serde(flatten)]
    changed: &'a TurnChanged,
    running: bool,
    needs_turn: bool,
    working: bool,
}

/// Quiet: callers have already emitted `session:turn_changed`.
async fn persist_turn(store: &Arc<Store>, changed: &TurnChanged) -> AppResult<()> {
    let session_id = changed.session_id.clone();
    let runtime_state = changed.runtime_state;
    let notification_type = changed.notification_type.clone();
    let acknowledged = changed.turn_acknowledged;
    store
        .update_workspace_quiet(&changed.workspace_id, move |ws| {
            if let Some(meta) = ws.session_mut(&session_id) {
                meta.runtime_state = Some(runtime_state);
                meta.notification_type = notification_type;
                meta.turn_acknowledged = acknowledged;
            }
            Ok(())
        })
        .await
}

fn session_ready_hook(app: AppHandle, workspace_id: String) -> crate::pty::OnReady {
    Box::new(move || {
        let _ = app.emit(
            "session:changed",
            serde_json::json!({ "workspace_id": workspace_id }),
        );
    })
}

fn session_exit_hook(
    app: AppHandle,
    store: Arc<Store>,
    turn: Arc<TurnTracker>,
    workspace_id: String,
    session_id: SessionId,
) -> OnExit {
    Box::new(move |code, ring| {
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

        if let Some(changed) = turn.observe(
            &session_id,
            &workspace_id,
            TurnSignal::ChildExited,
        ) {
            publish_turn(&app, &changed, false);
            let store = store.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = persist_turn(&store, &changed).await {
                    warn!(error = %e, "persist dormant turn state failed");
                }
            });
        }
    })
}

/// Drop the `[detached (from session …)]` line the tmux client prints as it
/// exits, so it doesn't greet the user on revisit.
fn trim_detach_epilogue(ring: &Ring) {
    const NEEDLE: &[u8] = b"[detached ";
    const SCAN_WINDOW: usize = 256;

    let mut ring = ring.lock().unwrap();
    if ring.is_empty() {
        return;
    }
    let tail_start = ring.len().saturating_sub(SCAN_WINDOW);
    let bytes = ring.make_contiguous();
    let Some(rel) = bytes[tail_start..]
        .windows(NEEDLE.len())
        .rposition(|w| w == NEEDLE)
    else {
        return;
    };
    let mut cut_from = tail_start + rel;
    while cut_from > 0 && matches!(bytes[cut_from - 1], b'\r' | b'\n') {
        cut_from -= 1;
    }
    ring.truncate(cut_from);
}

struct ProbeView<'a> {
    sid: &'a str,
    cwd: Option<&'a str>,
    state: SessionRuntimeState,
    status_updated_at: Option<i64>,
}

struct TrackedSession<'a> {
    workspace_id: &'a str,
    session_id: &'a SessionId,
    cwd: Option<&'a str>,
    agent_session_id: Option<&'a str>,
    running: bool,
}

#[derive(Debug, PartialEq)]
struct ProbeAction {
    workspace_id: String,
    session_id: SessionId,
    state: SessionRuntimeState,
    heal_to: Option<String>,
}

/// A rotated session id leaves a ghost probe behind in the same cwd. Left in,
/// it would win correlation and keep the stale id "live", blocking the heal.
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

/// Correlates by session id, else by the one running session in the probe's
/// cwd whose stored id has rotated away, healing it to the probe's.
fn plan_probe_reconciliation(
    probes: &[ProbeView],
    sessions: &[TrackedSession],
) -> Vec<ProbeAction> {
    let probes = freshest_probe_per_cwd(probes);
    let live_sids: HashSet<&str> = probes.iter().map(|p| p.sid).collect();
    let mut out = Vec::new();
    for p in probes {
        if let Some(s) =
            sessions.iter().find(|s| s.agent_session_id == Some(p.sid))
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
                && s.agent_session_id.is_none_or(|c| !live_sids.contains(c))
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

pub struct OpenSession<'a> {
    pub supervisor: &'a Arc<SessionSupervisor>,
    pub store: &'a Arc<Store>,
    pub workspace_id: &'a str,
    pub agent_bins: &'a AgentBins,
    pub tmux_bin: &'a Path,
    pub paths: &'a Paths,
    pub mcp: Option<&'a McpLaunch>,
    /// Only for the session a handoff creates.
    pub brief: Option<&'a str>,
}

/// Does the least that works: the running handle, a reattached tmux pane, a
/// resumed conversation, else a fresh start. The last two mint a new session
/// id, since the id is the tmux session name.
pub async fn open_session(req: OpenSession<'_>) -> AppResult<SessionInfo> {
    if req.tmux_bin.as_os_str().is_empty() {
        return Err(AppError::Other(
            "tmux not found — install with `brew install tmux` and restart Tethys".into(),
        ));
    }

    let (existing, cwd, agent, ws_binary, repo_keys) = req
        .store
        .read(|s| {
            let w = s.find_workspace(req.workspace_id)?;
            Some((
                w.session.clone(),
                w.session_cwd(),
                w.agent,
                w.agent_binary.clone(),
                w.repo_links
                    .iter()
                    .map(|l| l.repo_key.clone())
                    .collect::<Vec<_>>(),
            ))
        })
        .await
        .ok_or_else(|| AppError::WorkspaceNotFound(req.workspace_id.to_string()))?;
    let cwd = cwd.ok_or_else(|| {
        AppError::Other(format!(
            "workspace {} has no repos — nowhere to run {}",
            req.workspace_id,
            agent.label()
        ))
    })?;

    if let Some(prev) = &existing {
        if let Some(info) = req.supervisor.info(&prev.id).filter(|i| i.running) {
            return Ok(info);
        }
        if tmux::has_session(req.tmux_bin, &prev.id) {
            info!(session_id = %prev.id, "reattaching to live tmux session");
            return req.supervisor.reattach_tmux(
                prev.id.clone(),
                req.workspace_id.to_string(),
                &cwd,
                req.tmux_bin,
            );
        }
    }

    // Both CLIs report a session id before writing any transcript, and
    // resuming that fails with "No conversation found".
    let resume_sid = existing.as_ref().and_then(|prev| {
        prev.agent_session_id
            .as_deref()
            .filter(|_| transcript_is_resumable(prev.transcript_path.as_deref()))
    });

    let resolved_bin = match ws_binary.as_deref() {
        Some(bin) => crate::agent_bin::resolve_named(bin)?,
        None => req.agent_bins.get(agent).to_path_buf(),
    };

    if agent == Agent::Codex {
        crate::codex_trust::trust_or_warn(req.paths, &cwd);
    }

    // A worktree's git dir lives outside the workspace root.
    let extra_writable: Vec<PathBuf> = repo_keys
        .iter()
        .map(|key| req.paths.repo_git_dir(key))
        .collect();

    let (info, _token) = req.supervisor.spawn_agent(SpawnAgent {
        agent,
        workspace_id: req.workspace_id.to_string(),
        cwd: &cwd,
        tmux_bin: req.tmux_bin,
        agent_bin: &resolved_bin,
        extra_writable: &extra_writable,
        resume_session_id: resume_sid,
        mcp: req.mcp,
        brief: req.brief,
    })?;

    let meta = AgentSessionMeta {
        id: info.id.clone(),
        cwd,
        agent_session_id: None,
        transcript_path: None,
        runtime_state: None,
        notification_type: None,
        turn_acknowledged: false,
        last_active_at: Some(Utc::now()),
    };
    req.store
        .update_workspace(req.workspace_id, |ws| {
            ws.session = Some(meta);
            Ok(())
        })
        .await?;
    if let Some(prev) = existing {
        req.supervisor.forget(&prev.id);
    }

    Ok(info)
}

fn transcript_is_resumable(path: Option<&Path>) -> bool {
    path.and_then(|p| std::fs::metadata(p).ok())
        .map(|m| m.is_file() && m.len() > 0)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::{parent_session_from_subagent_path, transcript_is_resumable};
    use super::{plan_probe_reconciliation, ProbeAction, ProbeView, TrackedSession};
    use crate::state::SessionRuntimeState;

    #[test]
    fn transcript_none_is_not_resumable() {
        assert!(!transcript_is_resumable(None));
    }

    #[test]
    fn missing_transcript_is_not_resumable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.jsonl");
        assert!(!transcript_is_resumable(Some(&path)));
    }

    #[test]
    fn empty_transcript_is_not_resumable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.jsonl");
        std::fs::File::create(&path).unwrap();
        assert!(!transcript_is_resumable(Some(&path)));
    }

    #[test]
    fn nonempty_transcript_is_resumable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("convo.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, r#"{{"type":"user","message":"hi"}}"#).unwrap();
        assert!(transcript_is_resumable(Some(&path)));
    }

    #[test]
    fn stale_ghost_probe_does_not_block_healing() {
        let stored_id = "b6a26662".to_string();
        let sess_id = "tethys-sess".to_string();
        let ws_id = "ws".to_string();
        let cwd = "/wt/custom-fill-in-field/nl-ai";
        let probes = [
            ProbeView {
                sid: "b6a26662",
                cwd: Some(cwd),
                state: SessionRuntimeState::Working,
                status_updated_at: Some(1_784_158_540_640),
            },
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
            agent_session_id: Some(&stored_id),
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
            agent_session_id: Some(&stored_id),
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
            agent_session_id: Some(&stored_id),
            running: false,
        }];
        assert!(plan_probe_reconciliation(&probes, &sessions).is_empty());
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

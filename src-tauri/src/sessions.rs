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
use crate::mcp::McpLaunch;
use crate::pty::{OnExit, PtyProcess, PtySpawn, Ring};
use crate::state::{ClaudeSessionMeta, SessionRuntimeState};
use crate::store::Store;
use crate::turn::{TurnChanged, TurnSignal, TurnState, TurnTracker};
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
    /// Why this session is starting — `Spawned` for a fresh prompt,
    /// `Reattached` for a pane that may be mid-response. The turn state each
    /// implies is `TurnTracker`'s business, not the caller's.
    seed: TurnSignal,
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
    /// Whether this session wants the user's attention.
    ///
    /// Derived here rather than in the frontend, which used to recompute it in
    /// four places from `running` / `runtime_state` / `turn_acknowledged` —
    /// and two of those four disagreed about whether `running` mattered, so
    /// the sidebar aggregate and the chip dot could light differently for the
    /// same session.
    pub needs_turn: bool,
    /// Whether Claude is actively working in this session. Derived alongside
    /// `needs_turn` for the same reason.
    pub working: bool,
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

pub struct SessionSupervisor {
    sessions: Mutex<HashMap<SessionId, SessionHandle>>,
    /// Maps the `TETHYS_SPAWN_TOKEN` we set on the PTY env to the
    /// session metadata we need to update once Claude's SessionStart hook
    /// tells us the claude_session_id.
    pending: Mutex<HashMap<String, PendingSpawn>>,
    /// Owns every rule about what a session's turn indicator shows.
    /// `Arc` so the PTY exit hook can report a child exit without holding a
    /// reference back to the supervisor.
    turn: Arc<TurnTracker>,
    store: Arc<Store>,
    app: AppHandle,
}

impl SessionSupervisor {
    pub fn new(app: AppHandle, store: Arc<Store>) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            turn: Arc::new(TurnTracker::new()),
            store,
            app,
        }
    }

    /// Restore a session's turn state from its persisted snapshot at boot.
    ///
    /// Must run after `reattach_tmux`, which seeds `Working` for a pane that
    /// may be mid-response; the persisted value is the better answer when we
    /// have one.
    pub fn restore_turn(
        &self,
        session_id: &str,
        state: SessionRuntimeState,
        notification_type: Option<String>,
        acknowledged: bool,
    ) {
        // Seeds publish nothing: the frontend isn't subscribed yet and
        // `list_sessions` reads straight out of the tracker.
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

    /// Feed a signal to the turn tracker and, if it changed anything the user
    /// can see, tell the UI and write it through to `state.json`.
    ///
    /// The single place turn state becomes visible. Every source — hooks, the
    /// probe loop, the exit watcher, the user's acknowledgement — goes through
    /// here, so "what happens when two of them disagree" is answered by
    /// `TurnTracker::observe` rather than by whichever call site ran last.
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

    /// The user dismissed the "your turn" indicator. Cleared again by the
    /// next fresh signal — see `TurnTracker`.
    pub async fn acknowledge_turn(&self, session_id: &str, workspace_id: &str) {
        self.apply_signal(session_id, workspace_id, TurnSignal::Acknowledged)
            .await;
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
            seed,
        } = req;
        let seed_state = match seed {
            TurnSignal::Reattached => SessionRuntimeState::Working,
            _ => SessionRuntimeState::WaitingInput,
        };
        let info = SessionInfo {
            id: id.clone(),
            workspace_id: workspace_id.clone(),
            repo_key,
            cwd: cwd.to_path_buf(),
            running: true,
            runtime_state: seed_state,
            notification_type: None,
            turn_acknowledged: false,
            needs_turn: false,
            working: false,
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
        // Seeds publish nothing — hooks refine the state moments later, and
        // the frontend reads the seed out of `list_sessions`.
        self.turn.observe(&id, &workspace_id, seed);
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
    ///
    /// `mcp` puts the handoff tool in this session's hands; `brief` is the
    /// first message it starts with, set only for the session a handoff
    /// creates.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_claude(
        &self,
        workspace_id: String,
        repo_key: Option<String>,
        cwd: &Path,
        tmux_bin: &Path,
        claude_bin: &Path,
        resume_claude_session_id: Option<&str>,
        mcp: Option<&McpLaunch>,
        brief: Option<&str>,
    ) -> AppResult<(SessionInfo, String)> {
        let token = Uuid::new_v4().to_string();
        let id = new_session_id();

        let mut command = vec![claude_bin.to_string_lossy().into_owned()];
        if let Some(csid) = resume_claude_session_id {
            command.push("--resume".into());
            command.push(csid.to_string());
        }
        // Rendered here rather than by the caller because the config bakes in
        // the session id, and this is where that id is minted.
        if let Some(mcp) = mcp {
            command.extend(mcp.claude_args(&workspace_id, &id));
        }
        // The Brief goes last: `claude [options] [prompt]`. tmux passes argv
        // through verbatim, so a multi-line brief with quotes in it survives.
        if let Some(brief) = brief {
            command.push(brief.to_string());
        }
        let args = tmux::new_session_args(
            &id,
            &[("TETHYS_SPAWN_TOKEN", token.clone())],
            &command,
        );

        let info = self.spawn_with_id(SpawnRequest {
            id,
            workspace_id: workspace_id.clone(),
            repo_key,
            cwd,
            program: tmux_bin,
            args: &args,
            tmux_bin: tmux_bin.to_path_buf(),
            seed_bytes: &[],
            seed: TurnSignal::Spawned,
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

        let args = tmux::attach_session_args(&session_id);
        self.spawn_with_id(SpawnRequest {
            id: session_id,
            workspace_id,
            repo_key,
            cwd,
            program: tmux_bin,
            args: &args,
            tmux_bin: tmux_bin.to_path_buf(),
            seed_bytes: &seed,
            seed: TurnSignal::Reattached,
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

    /// Find the Tethys session this hook belongs to and feed the tracker.
    ///
    /// Matches first on `claude_session_id`. Falls back to the parent session
    /// when the hook comes from a subagent — subagent transcripts live at
    /// `.../<parent-uuid>/subagents/agent-*.jsonl`, so the parent's id is
    /// recoverable from `transcript_path`. Falls back last to `cwd`, which is
    /// stable across the id rotation Claude does on compaction/resume; without
    /// it a rotated id means every hook silently misses until the 2s probe
    /// loop heals the id.
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
        let cwd = msg.cwd.as_deref();
        let lookup = self
            .store
            .read(|s| {
                let mut by_cwd = None;
                for ws in &s.workspaces {
                    for sess in &ws.sessions {
                        let tracked = sess.claude_session_id.as_deref();
                        if tracked == Some(csid)
                            || (parent_csid.is_some()
                                && tracked == parent_csid.as_deref())
                        {
                            return Some((ws.id.clone(), sess.id.clone()));
                        }
                        // Remember a cwd match but keep looking for an id
                        // match, which is always the better answer.
                        if by_cwd.is_none()
                            && cwd.is_some()
                            && sess.cwd.to_str() == cwd
                        {
                            by_cwd = Some((ws.id.clone(), sess.id.clone()));
                        }
                    }
                }
                by_cwd
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
        let sessions = self.sessions.lock().unwrap();
        sessions
            .values()
            .filter(|h| h.info.workspace_id == workspace_id)
            .map(|h| {
                let mut info = h.info.clone();
                info.running = h.pty.is_running();
                let turn = self.turn.get(&h.info.id);
                info.needs_turn = turn.needs_turn(info.running);
                info.working = turn.is_working(info.running);
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

/// Emit a turn change to the frontend. One shape, one place — the payload
/// used to be hand-built as `serde_json::json!` at three call sites, and the
/// third had already drifted, omitting `turn_acknowledged` while the
/// TypeScript type declared it non-optional.
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

/// What goes over the wire on `session:turn_changed`.
///
/// The tracker is pure and can't know whether the PTY is still alive, so the
/// two liveness-dependent predicates are added here — the one place with
/// access to both. The frontend gets the answer rather than the ingredients.
#[derive(Clone, Serialize)]
struct TurnChangedEvent<'a> {
    #[serde(flatten)]
    changed: &'a TurnChanged,
    running: bool,
    needs_turn: bool,
    working: bool,
}

/// Write a turn change through to `state.json` so the indicator survives a
/// restart. Quiet: the caller already emitted the more specific
/// `session:turn_changed`, so there's no need for a `workspace:changed` too.
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

/// Build the exit hook handed to [`PtyProcess::spawn`]. It runs only on a
/// true child exit (the watcher already filtered out client detaches): scrub
/// tmux's detach epilogue from the ring, announce the exit, and record the
/// session as `Dormant`.
///
/// Recording it is the part that used to be missing. The hook emitted a
/// `Dormant` event but never wrote it anywhere, so `list_sessions` kept
/// returning the pre-exit state — and the frontend's own refresh, racing the
/// event, put the stale state straight back and re-lit the sidebar dot for a
/// session that had already died.
fn session_exit_hook(
    app: AppHandle,
    store: Arc<Store>,
    turn: Arc<TurnTracker>,
    workspace_id: String,
    session_id: SessionId,
) -> OnExit {
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

        if let Some(changed) = turn.observe(
            &session_id,
            &workspace_id,
            TurnSignal::ChildExited,
        ) {
            // The child just exited, so it is definitively not running.
            publish_turn(&app, &changed, false);
            // The exit hook is a sync callback on the watcher thread, so the
            // write-through goes on the runtime.
            let store = store.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = persist_turn(&store, &changed).await {
                    warn!(error = %e, "persist dormant turn state failed");
                }
            });
        }
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

/// One request to put a Claude session on screen, however it got there:
/// a fresh start, a resume, a binary switch, or the session a handoff creates.
pub struct StartSession<'a> {
    pub supervisor: &'a Arc<SessionSupervisor>,
    pub store: &'a Arc<Store>,
    pub workspace_id: &'a str,
    /// `None` => start at the workspace root (the parent dir containing each
    /// repo's worktree subdir).
    pub repo_key: Option<String>,
    /// App-wide binary resolved at boot; the fallback when neither the session
    /// nor the workspace overrides it.
    pub claude_bin: &'a Path,
    pub tmux_bin: &'a Path,
    /// Handoff tool config. Attached to every session Tethys spawns — whether
    /// an agent can hand off shouldn't depend on which workspace it landed in.
    pub mcp: Option<&'a McpLaunch>,
    /// Resume an existing conversation via `claude --resume <id>`.
    pub resume_claude_sid: Option<&'a str>,
    /// Per-session binary override to run under and persist onto the new meta.
    /// Takes precedence over the workspace default; `None` falls back to it.
    pub session_binary: Option<&'a str>,
    /// The Brief, for the session a handoff creates. `None` everywhere else —
    /// a session the user started is one they're about to type into.
    pub brief: Option<&'a str>,
}

/// Resolve where the session runs and which binary it runs under, spawn it,
/// and persist the `ClaudeSessionMeta` that makes it resumable across restarts.
pub async fn start_session(req: StartSession<'_>) -> AppResult<SessionInfo> {
    if req.tmux_bin.as_os_str().is_empty() {
        return Err(AppError::Other(
            "tmux not found — install with `brew install tmux` and restart Tethys".into(),
        ));
    }

    // Resolve the cwd: a specific repo's worktree, or — when repo_key is
    // None — the workspace root (parent of every repo worktree).
    // Also pull the per-workspace claude binary override, if any.
    let (cwd, ws_binary) = req
        .store
        .read(|s| {
            let w = s.find_workspace(req.workspace_id)?;
            let cwd = match req.repo_key.as_deref() {
                Some(key) => w.link(key).map(|r| r.worktree_path.clone()),
                None => w.root_buf(),
            }?;
            Some((cwd, w.claude_binary.clone()))
        })
        .await
        .ok_or_else(|| {
            AppError::Other(match req.repo_key.as_deref() {
                Some(key) => format!("no worktree for {}/{} in state", req.workspace_id, key),
                None => format!(
                    "workspace {} has no repos — can't resolve a root dir",
                    req.workspace_id
                ),
            })
        })?;

    // Session override wins over the workspace default, which wins over the
    // app-wide binary resolved at boot.
    let resolved_bin = match req.session_binary.or(ws_binary.as_deref()) {
        Some(bin) => crate::claude::resolve_named(bin)?,
        None => req.claude_bin.to_path_buf(),
    };

    let (info, _token) = req.supervisor.spawn_claude(
        req.workspace_id.to_string(),
        req.repo_key.clone(),
        &cwd,
        req.tmux_bin,
        &resolved_bin,
        req.resume_claude_sid,
        req.mcp,
        req.brief,
    )?;

    // Persist a ClaudeSessionMeta entry so resume works across restarts.
    // claude_session_id is filled in by the SessionStart hook once it
    // arrives. We key on the Tethys-internal `id` (== SessionSupervisor id)
    // so the UI and supervisor use a shared identifier.
    let meta = ClaudeSessionMeta {
        id: info.id.clone(),
        repo_key: req.repo_key.clone(),
        cwd: cwd.clone(),
        claude_session_id: None,
        transcript_path: None,
        claude_binary: req.session_binary.map(str::to_string),
        hidden: false,
        runtime_state: None,
        notification_type: None,
        turn_acknowledged: false,
    };

    req.store
        .update_workspace(req.workspace_id, |ws| {
            // Resuming? Drop the prior meta for this Claude conversation so
            // we don't accumulate dormant duplicates with the same
            // claude_session_id across runs.
            if let Some(csid) = req.resume_claude_sid {
                ws.sessions
                    .retain(|m| m.claude_session_id.as_deref() != Some(csid));
            }
            // Defensive: no dupes of the new tethys id either.
            ws.sessions.retain(|m| m.id != meta.id);
            ws.sessions.push(meta);
            Ok(())
        })
        .await?;

    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::parent_session_from_subagent_path;
    use super::{plan_probe_reconciliation, ProbeAction, ProbeView, TrackedSession};
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

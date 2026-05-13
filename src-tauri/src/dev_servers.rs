//! Glue between Tauri commands, the dev orchestrator, and Tethys's
//! session supervisor. Handles the actual lifecycle of dev-server
//! sessions (spawn into tmux, persist `DevServersMeta`, kill on stop)
//! while delegating port allocation / override file / env links to
//! `dev_orchestrator`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex as StdMutex};

use chrono::Utc;
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex as AsyncMutex;
use tracing::{info, warn};

use crate::dev_orchestrator::{self, BeMode, OrchestratorConfig, PrepResult, ServicePrep};
use crate::error::{AppError, AppResult};
use crate::sessions::SessionSupervisor;
use crate::state::{ClaudeSessionMeta, DevServersMeta, SessionKind, WorkspaceId};
use crate::store::Store;
use crate::tmux::{self, TmuxBin};

/// Per-workspace mutex registry. Concurrent start/stop calls on the
/// same workspace serialize through one of these; different workspaces
/// can proceed in parallel.
pub struct DevServerLocks {
    locks: StdMutex<HashMap<String, Arc<AsyncMutex<()>>>>,
}

impl DevServerLocks {
    pub fn new() -> Self {
        Self {
            locks: StdMutex::new(HashMap::new()),
        }
    }

    fn for_workspace(&self, id: &str) -> Arc<AsyncMutex<()>> {
        let mut g = self.locks.lock().unwrap();
        g.entry(id.to_string())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone()
    }
}

impl Default for DevServerLocks {
    fn default() -> Self {
        Self::new()
    }
}

/// Live state of one dev-server service (FE or BE).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ServiceLiveState {
    pub session_id: Option<String>,
    pub running: bool,
    pub port: Option<u16>,
}

/// What the frontend gets when asking "is dev up?".
#[derive(Debug, Clone, serde::Serialize)]
pub struct DevStateSnapshot {
    pub workspace_id: WorkspaceId,
    pub fe: Option<ServiceLiveState>,
    pub be: Option<ServiceLiveState>,
    /// Echoed from `DevServersMeta` so the UI can show "FE → master" vs
    /// "FE → this worktree".
    pub fe_proxy_target: Option<String>,
}

/// Start dev servers for a workspace. Per-workspace serialized.
#[allow(clippy::too_many_arguments)]
pub async fn start(
    app: &AppHandle,
    supervisor: &Arc<SessionSupervisor>,
    store: &Arc<Store>,
    tmux_bin: &TmuxBin,
    locks: &Arc<DevServerLocks>,
    workspace_id: &str,
    mode: BeMode,
    cfg: &Arc<OrchestratorConfig>,
) -> AppResult<DevServersMeta> {
    if tmux_bin.0.as_os_str().is_empty() {
        return Err(AppError::Other(
            "tmux not found — install with `brew install tmux` and restart Tethys".into(),
        ));
    }
    let lock = locks.for_workspace(workspace_id);
    let _guard = lock.lock().await;

    // Snapshot the workspace for prep.
    let workspace = store
        .read(|s| s.find_workspace(workspace_id).cloned())
        .await
        .ok_or_else(|| AppError::WorkspaceNotFound(workspace_id.into()))?;

    let prep: PrepResult = dev_orchestrator::prep(cfg.as_ref(), &workspace, mode)
        .map_err(AppError::Other)?;

    // Spawn FE (always).
    spawn_one(supervisor, &tmux_bin.0, &cfg.fe_repo_key, &prep.fe)?;

    // Spawn BE (when prep says so).
    if let Some(be) = &prep.be {
        spawn_one(supervisor, &tmux_bin.0, &cfg.be_repo_key, be)?;
    }

    // If the main stack was just (re)started and our BE was already
    // running from a previous session, its postgres connections are
    // stale. Restart it (no-op if it didn't exist).
    if prep.main_stack_was_started {
        dev_orchestrator::restart_worktree_django(cfg.as_ref(), &prep.short);
    }

    let dev_meta = DevServersMeta {
        fe_port: prep.fe.port,
        be_port: prep.be.as_ref().map(|b| b.port),
        fe_session_id: Some(prep.fe.session_id.clone()),
        be_session_id: prep.be.as_ref().map(|b| b.session_id.clone()),
        fe_proxy_target: prep.fe_proxy_target.clone(),
        started_at: Utc::now(),
    };

    // Persist: drop any prior dev-server session metas (they were
    // either just respawned or are stale), then append the new ones
    // tagged with SessionKind::FrontendBuild / BackendBuild, then set
    // dev_servers.
    let fe_meta = ClaudeSessionMeta {
        id: prep.fe.session_id.clone(),
        kind: SessionKind::FrontendBuild,
        repo_key: Some(cfg.fe_repo_key.clone()),
        cwd: prep.fe.cwd.clone(),
        claude_session_id: None,
        transcript_path: None,
        hidden: false,
        runtime_state: None,
        notification_type: None,
        turn_acknowledged: false,
    };
    let be_meta = prep.be.as_ref().map(|b| ClaudeSessionMeta {
        id: b.session_id.clone(),
        kind: SessionKind::BackendBuild,
        repo_key: Some(cfg.be_repo_key.clone()),
        cwd: b.cwd.clone(),
        claude_session_id: None,
        transcript_path: None,
        hidden: false,
        runtime_state: None,
        notification_type: None,
        turn_acknowledged: false,
    });

    let wid = workspace_id.to_string();
    let dev_meta_clone = dev_meta.clone();
    store
        .mutate(move |s| {
            let ws = s
                .find_workspace_mut(&wid)
                .ok_or_else(|| AppError::WorkspaceNotFound(wid.clone()))?;
            // Drop any existing dev-server session metas — they're either
            // about to be replaced or were stale (tmux session long gone).
            ws.sessions.retain(|m| {
                !matches!(m.kind, SessionKind::FrontendBuild | SessionKind::BackendBuild)
            });
            ws.sessions.push(fe_meta);
            if let Some(meta) = be_meta {
                ws.sessions.push(meta);
            }
            ws.dev_servers = Some(dev_meta_clone);
            Ok(())
        })
        .await?;

    let _ = app.emit(
        "workspace:changed",
        serde_json::json!({ "workspace_id": workspace_id }),
    );
    info!(
        workspace = %workspace_id,
        label = %prep.label,
        fe_port = prep.fe.port,
        be_port = prep.be.as_ref().map(|b| b.port),
        proxy = %prep.fe_proxy_target,
        "dev servers started"
    );
    Ok(dev_meta)
}

fn spawn_one(
    supervisor: &Arc<SessionSupervisor>,
    tmux_bin: &Path,
    repo_key: &str,
    svc: &ServicePrep,
) -> AppResult<()> {
    supervisor
        .spawn_dev_server(
            svc.session_id.clone(),
            // workspace_id is encoded in the session_id; supervisor
            // stores it on the SessionHandle for emit purposes.
            workspace_id_from_session(&svc.session_id),
            Some(repo_key.to_string()),
            &svc.cwd,
            tmux_bin,
            &svc.shell_command,
            &svc.env,
        )
        .map(|_| ())
}

/// Extract the workspace_id from a `tethys-fe-<wid>` / `tethys-be-<wid>`
/// session id. Returns the whole id back if it doesn't match (defensive).
fn workspace_id_from_session(session_id: &str) -> String {
    if let Some(rest) = session_id.strip_prefix("tethys-fe-") {
        return rest.to_string();
    }
    if let Some(rest) = session_id.strip_prefix("tethys-be-") {
        return rest.to_string();
    }
    session_id.to_string()
}

/// Stop dev servers for a workspace. Per-workspace serialized.
pub async fn stop(
    app: &AppHandle,
    store: &Arc<Store>,
    tmux_bin: &TmuxBin,
    locks: &Arc<DevServerLocks>,
    workspace_id: &str,
    cfg: &Arc<OrchestratorConfig>,
) -> AppResult<()> {
    let lock = locks.for_workspace(workspace_id);
    let _guard = lock.lock().await;

    let workspace = store
        .read(|s| s.find_workspace(workspace_id).cloned())
        .await
        .ok_or_else(|| AppError::WorkspaceNotFound(workspace_id.into()))?;

    let _report = dev_orchestrator::stop(cfg.as_ref(), &workspace).map_err(AppError::Other)?;

    // Kill our tmux sessions explicitly (the docker + rspack stop above
    // doesn't touch them). Idempotent.
    let session_ids: Vec<String> = workspace
        .sessions
        .iter()
        .filter(|m| matches!(m.kind, SessionKind::FrontendBuild | SessionKind::BackendBuild))
        .map(|m| m.id.clone())
        .collect();
    if !tmux_bin.0.as_os_str().is_empty() {
        for sid in &session_ids {
            tmux::kill_session(&tmux_bin.0, sid);
        }
    }

    let wid = workspace_id.to_string();
    store
        .mutate(move |s| {
            let ws = s
                .find_workspace_mut(&wid)
                .ok_or_else(|| AppError::WorkspaceNotFound(wid.clone()))?;
            ws.sessions.retain(|m| {
                !matches!(m.kind, SessionKind::FrontendBuild | SessionKind::BackendBuild)
            });
            ws.dev_servers = None;
            Ok(())
        })
        .await?;

    let _ = app.emit(
        "workspace:changed",
        serde_json::json!({ "workspace_id": workspace_id }),
    );
    info!(workspace = %workspace_id, "dev servers stopped");
    Ok(())
}

/// Snapshot for the UI. Combines persisted `DevServersMeta` with live
/// supervisor state (which tmux sessions are still attached).
pub async fn snapshot(
    supervisor: &Arc<SessionSupervisor>,
    store: &Arc<Store>,
    workspace_id: &str,
) -> AppResult<DevStateSnapshot> {
    let snap = store
        .read(|s| s.find_workspace(workspace_id).cloned())
        .await
        .ok_or_else(|| AppError::WorkspaceNotFound(workspace_id.into()))?;
    let live = supervisor.list_for_workspace(workspace_id);
    let dev = snap.dev_servers.as_ref();
    let fe = dev.and_then(|d| d.fe_session_id.as_ref()).map(|sid| {
        let running = live.iter().any(|s| &s.id == sid && s.running);
        ServiceLiveState {
            session_id: Some(sid.clone()),
            running,
            port: Some(dev.unwrap().fe_port),
        }
    });
    let be = dev
        .and_then(|d| d.be_session_id.as_ref())
        .map(|sid| ServiceLiveState {
            session_id: Some(sid.clone()),
            running: live.iter().any(|s| &s.id == sid && s.running),
            port: dev.unwrap().be_port,
        });
    Ok(DevStateSnapshot {
        workspace_id: workspace_id.to_string(),
        fe,
        be,
        fe_proxy_target: dev.map(|d| d.fe_proxy_target.clone()),
    })
}

/// Quick helper for the auto-decide UI hint: does this workspace's BE
/// have any changes vs master? Cheap (one git diff).
pub async fn detect_be_changes(
    store: &Arc<Store>,
    workspace_id: &str,
    cfg: &Arc<OrchestratorConfig>,
) -> AppResult<bool> {
    let workspace = store
        .read(|s| s.find_workspace(workspace_id).cloned())
        .await
        .ok_or_else(|| AppError::WorkspaceNotFound(workspace_id.into()))?;
    let be_dir = workspace
        .repo_links
        .iter()
        .find(|l| l.repo_key == cfg.be_repo_key)
        .map(|l| l.worktree_path.clone())
        .ok_or_else(|| AppError::Other(format!("workspace has no '{}' repo linked", cfg.be_repo_key)))?;
    let out = std::process::Command::new("git")
        .args(["diff", "--quiet", &cfg.master_branch, "--", "."])
        .current_dir(&be_dir)
        .status()
        .map_err(|e| AppError::Other(format!("git invoke: {e}")))?;
    Ok(!out.success())
}


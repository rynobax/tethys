use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tauri::{AppHandle, Emitter};
use tokio::sync::Notify;
use tracing::{info, warn};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::git;
use crate::job::JobTx;
use crate::paths::Paths;
use crate::pending_permissions;
use crate::reconcile;
use crate::registry::RegistryLoad;
use crate::state::{SystemErrorEntry, Workspace};
use crate::store::Store;

/// How long a workspace must be soft-deleted before the purger will tear it
/// down. Prevents the cron from racing the user who just hit Delete.
const PURGE_GRACE: chrono::Duration = chrono::Duration::hours(1);

/// Hourly tick rate for the background purger.
const TICK_INTERVAL: Duration = Duration::from_secs(3600);

/// Tear down a soft-deleted workspace's worktrees, branches, and parent dir,
/// then drop it from `AppState`. No-op for workspaces without `deleted_at`.
///
/// Designed to run unattended from the background purger — no `JobEvent`
/// channel, no UI streaming. Errors propagate so the caller can record a
/// `SystemErrorEntry`.
pub async fn purge_workspace(
    store: &Arc<Store>,
    paths: &Paths,
    registry: &Arc<RegistryLoad>,
    workspace: &Workspace,
) -> AppResult<()> {
    // Capture any permission entries that exist in the combined file but
    // aren't accounted for by per-repo settings — these are grants the user
    // approved in a workspace-root Claude session. Best-effort: a failure
    // here is logged but doesn't block the purge itself, since blocking
    // would leave the user with a workspace they can't tear down.
    if let Err(e) = pending_permissions::capture_for_purge(workspace, paths).await {
        warn!(
            workspace = %workspace.id,
            error = %e,
            "failed to capture pending permissions before purge"
        );
    }

    // The purger runs in the background with no job channel to stream to, so
    // its git output is discarded — but failures still carry the stderr tail.
    let tx = JobTx::silent();

    for link in &workspace.repo_links {
        let clone_path = paths.repo_clone_path(&link.repo_key);

        if !link.worktree_path.exists() {
            continue;
        }
        if !clone_path.exists() {
            // Registry entry is gone or the clone was manually deleted —
            // remove the worktree dir directly.
            tokio::fs::remove_dir_all(&link.worktree_path)
                .await
                .map_err(|e| {
                    AppError::Other(format!(
                        "failed to remove {}: {e}",
                        link.worktree_path.display()
                    ))
                })?;
            continue;
        }

        git::worktree_remove(&clone_path, &link.worktree_path, true, &tx, &link.repo_key)
            .await?;
        git::worktree_prune_best_effort(&clone_path, &tx, &link.repo_key).await;
        // Only delete branches Tethys created — a pre-existing branch checked
        // out for local edits (e.g. a PR branch) must survive the purge.
        if link.created_branch {
            git::branch_delete_best_effort(&clone_path, &workspace.branch, &tx, &link.repo_key)
                .await;
        }
    }

    // Remove the parent dir left behind by `git worktree remove`.
    if let Ok(reg) = registry.require() {
        if let Some(parent) = workspace.root_buf() {
            if parent.exists() && reconcile::is_under(&reg.worktree_root, &parent) {
                if let Err(e) = tokio::fs::remove_dir_all(&parent).await {
                    warn!(path = %parent.display(), error = %e, "failed to remove workspace dir during purge");
                }
            }
        }
    }

    let id = workspace.id.clone();
    store
        .mutate(|s| {
            s.workspaces.retain(|w| w.id != id);
            Ok(())
        })
        .await
}

pub struct Purger {
    store: Arc<Store>,
    paths: Paths,
    registry: Arc<RegistryLoad>,
    app: AppHandle,
    force: Arc<Notify>,
}

impl Purger {
    pub fn new(
        store: Arc<Store>,
        paths: Paths,
        registry: Arc<RegistryLoad>,
        app: AppHandle,
    ) -> Self {
        Self {
            store,
            paths,
            registry,
            app,
            force: Arc::new(Notify::new()),
        }
    }

    /// Long-running loop. Spawn with `tokio::spawn(purger.run())`.
    pub async fn run(self: Arc<Self>) {
        // Run an initial tick on startup so leftover deletions from a prior
        // session that crossed the grace window get cleaned up promptly.
        self.tick().await;
        loop {
            tokio::select! {
                _ = tokio::time::sleep(TICK_INTERVAL) => {}
                _ = self.force.notified() => {}
            }
            self.tick().await;
        }
    }

    /// Trigger an immediate tick (the "Run cleanup now" button).
    pub fn request_tick(&self) {
        self.force.notify_one();
    }

    async fn tick(self: &Arc<Self>) {
        let cutoff = Utc::now() - PURGE_GRACE;
        let candidates: Vec<Workspace> = self
            .store
            .read(|s| {
                s.workspaces
                    .iter()
                    .filter(|w| w.deleted_at.is_some_and(|t| t <= cutoff))
                    .cloned()
                    .collect()
            })
            .await;

        if candidates.is_empty() {
            return;
        }

        info!(n = candidates.len(), "purger tick: candidates");
        let mut any_change = false;
        for ws in candidates {
            match purge_workspace(&self.store, &self.paths, &self.registry, &ws).await {
                Ok(_) => {
                    info!(id = %ws.id, branch = %ws.branch, "purged workspace");
                    any_change = true;
                    let _ = self.app.emit(
                        "workspace:changed",
                        serde_json::json!({ "workspace_id": ws.id }),
                    );
                }
                Err(e) => {
                    let msg = e.to_string();
                    warn!(id = %ws.id, branch = %ws.branch, error = %msg, "purge failed");
                    record_system_error(
                        &self.store,
                        SystemErrorEntry {
                            id: Uuid::new_v4().to_string(),
                            at: Utc::now(),
                            kind: "purge".into(),
                            message: msg,
                            workspace_id: Some(ws.id.clone()),
                            workspace_branch: Some(ws.branch.clone()),
                        },
                    )
                    .await;
                    any_change = true;
                }
            }
        }

        if any_change {
            let _ = self.app.emit("system_status:changed", &());
            // Purge captures workspace-root permission grants into
            // pending_permissions.json. The capture may have appended new
            // entries, so nudge any open modal to refresh.
            let _ = self.app.emit("pending_permissions:changed", &());
        }
    }
}

pub async fn record_system_error(store: &Arc<Store>, entry: SystemErrorEntry) {
    let _ = store
        .mutate(|s| {
            s.system_errors.push(entry);
            // Cap the log so a stuck workspace doesn't grow state.json forever.
            const MAX_ENTRIES: usize = 200;
            if s.system_errors.len() > MAX_ENTRIES {
                let drop = s.system_errors.len() - MAX_ENTRIES;
                s.system_errors.drain(0..drop);
            }
            Ok(())
        })
        .await;
}


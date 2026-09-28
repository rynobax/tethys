use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tauri::{AppHandle, Emitter};
use tokio::sync::Notify;
use tracing::{info, warn};
use uuid::Uuid;

use crate::agent::Agent;
use crate::error::{AppError, AppResult};
use crate::git;
use crate::job::JobTx;
use crate::paths::Paths;
use crate::pending_permissions;
use crate::reconcile;
use crate::registry::RegistryLoad;
use crate::state::{SystemErrorEntry, Workspace};
use crate::store::Store;

/// Keeps the purger from racing a user who just hit Delete and might undo it.
const PURGE_GRACE: chrono::Duration = chrono::Duration::hours(1);

const TICK_INTERVAL: Duration = Duration::from_secs(3600);

pub async fn purge_workspace(
    store: &Arc<Store>,
    paths: &Paths,
    registry: &Arc<RegistryLoad>,
    workspace: &Workspace,
) -> AppResult<()> {
    // Best-effort: failing here must not leave a workspace that can't be torn down.
    if let Err(e) = pending_permissions::capture_for_purge(workspace, paths).await {
        warn!(
            workspace = %workspace.id,
            error = %e,
            "failed to capture pending permissions before purge"
        );
    }

    let tx = JobTx::silent();

    for link in &workspace.repo_links {
        let clone_path = paths.repo_clone_path(&link.repo_key);

        if !link.worktree_path.exists() {
            continue;
        }
        if !clone_path.exists() {
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
        // A pre-existing branch (e.g. someone's PR) must survive the purge.
        if link.created_branch {
            git::branch_delete_best_effort(&clone_path, &workspace.branch, &tx, &link.repo_key)
                .await;
        }
    }

    if let Ok(reg) = registry.require() {
        if let Some(parent) = workspace.root_buf() {
            if parent.exists() && reconcile::is_under(&reg.worktree_root, &parent) {
                if let Err(e) = tokio::fs::remove_dir_all(&parent).await {
                    warn!(path = %parent.display(), error = %e, "failed to remove workspace dir during purge");
                }
            }
        }
    }

    if workspace.agent == Agent::Codex {
        if let Some(root) = workspace.session_cwd().or_else(|| workspace.root_buf()) {
            crate::codex_trust::untrust_or_warn(paths, &root);
        }
    }

    let id = workspace.id.clone();
    store
        .mutate(|s| {
            s.workspaces.retain(|w| w.id != id);
            s.clear_links_to(&id);
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

    pub async fn run(self: Arc<Self>) {
        self.tick().await;
        loop {
            tokio::select! {
                _ = tokio::time::sleep(TICK_INTERVAL) => {}
                _ = self.force.notified() => {}
            }
            self.tick().await;
        }
    }

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
                    self.store.notify_changed(&ws.id);
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
            let _ = self.app.emit("pending_permissions:changed", &());
        }
    }
}

pub async fn record_system_error(store: &Arc<Store>, entry: SystemErrorEntry) {
    let _ = store
        .mutate(|s| {
            s.system_errors.push(entry);
            // A stuck workspace would otherwise grow state.json every tick.
            const MAX_ENTRIES: usize = 200;
            if s.system_errors.len() > MAX_ENTRIES {
                let drop = s.system_errors.len() - MAX_ENTRIES;
                s.system_errors.drain(0..drop);
            }
            Ok(())
        })
        .await;
}


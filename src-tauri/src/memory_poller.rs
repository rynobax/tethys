//! Periodic memory + dev-server-state poller. Emits
//! `devserver:memory_updated` to the frontend every 5s with the
//! system pressure level + per-workspace RAM. Cost budget: ~100ms
//! per tick (`docker stats` dominates).

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::Notify;
use tracing::trace;

use crate::dev_orchestrator::{self, OrchestratorConfig};
use crate::dev_orchestrator::stats::WorkspaceMemory;
use crate::store::Store;

const POLL_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Serialize)]
pub struct MemorySnapshot {
    pub system: dev_orchestrator::SystemMemory,
    pub per_workspace: Vec<WorkspaceMemory>,
}

pub struct MemoryPoller {
    store: Arc<Store>,
    cfg: Arc<OrchestratorConfig>,
    app: AppHandle,
    force: Arc<Notify>,
}

impl MemoryPoller {
    pub fn new(store: Arc<Store>, cfg: Arc<OrchestratorConfig>, app: AppHandle) -> Arc<Self> {
        Arc::new(Self {
            store,
            cfg,
            app,
            force: Arc::new(Notify::new()),
        })
    }

    /// Force an immediate tick (skip the sleep). Use after `start_dev_servers`
    /// so the UI updates quickly without waiting up to 5s.
    pub fn request_tick(&self) {
        self.force.notify_one();
    }

    pub async fn run(self: Arc<Self>) {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(POLL_INTERVAL) => {}
                _ = self.force.notified() => {}
            }
            self.tick().await;
        }
    }

    async fn tick(&self) {
        let snap = snapshot_now(&self.store, &self.cfg).await;
        trace!(
            level = ?snap.system.level,
            free_pct = snap.system.free_pct,
            workspaces = snap.per_workspace.len(),
            "memory tick"
        );
        let _ = self.app.emit("devserver:memory_updated", &snap);
    }
}

/// Build a fresh snapshot. Exposed so the `get_memory_snapshot` Tauri
/// command can return one on demand (UI mount), independent of the
/// poller's cadence.
pub async fn snapshot_now(store: &Arc<Store>, cfg: &Arc<OrchestratorConfig>) -> MemorySnapshot {
    let workspaces: Vec<(String, String, std::path::PathBuf)> = store
        .read(|s| {
            s.workspaces
                .iter()
                .filter(|w| w.deleted_at.is_none())
                .filter_map(|w| {
                    // Need both an FE worktree (for rspack RAM lookup) and
                    // a short suffix from the branch (for the container
                    // name lookup). Workspaces without a configured FE
                    // repo are skipped — they wouldn't have a dev stack
                    // anyway.
                    let fe = w
                        .repo_links
                        .iter()
                        .find(|l| l.repo_key == cfg.fe_repo_key)?
                        .worktree_path
                        .clone();
                    let short = dev_orchestrator::short_from_branch(&w.branch);
                    Some((w.id.clone(), short, fe))
                })
                .collect()
        })
        .await;
    let per_workspace = dev_orchestrator::stats::workspace_memory(cfg.as_ref(), &workspaces);
    let system = dev_orchestrator::pressure::current();
    MemorySnapshot {
        system,
        per_workspace,
    }
}

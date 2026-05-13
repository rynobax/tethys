//! Sweep stale `docker-compose.override.yml` from worktrees that no
//! longer exist on disk. Cheap (a few `stat`/`git rev-parse` calls);
//! runs before we add a new one so the worktree-root dir doesn't
//! accumulate cruft as branches come and go.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tracing::info;

use super::config::OrchestratorConfig;

/// Returns the paths that were removed.
pub fn scrub_orphan_overrides(cfg: &OrchestratorConfig) -> Vec<PathBuf> {
    let mut removed = Vec::new();
    let Ok(entries) = fs::read_dir(&cfg.worktree_root) else {
        return removed;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let be_dir = path.join(&cfg.be_repo_key);
        let override_file = be_dir.join("docker-compose.override.yml");
        if !override_file.exists() {
            continue;
        }
        if is_orphan(&be_dir) {
            if fs::remove_file(&override_file).is_ok() {
                info!(path = %override_file.display(), "scrubbed orphan override");
                removed.push(override_file);
            }
        }
    }
    removed
}

/// A BE dir counts as "orphaned" if it's gone, or if it's not a valid
/// git worktree any longer.
fn is_orphan(be_dir: &Path) -> bool {
    if !be_dir.exists() {
        return true;
    }
    let ok = Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(be_dir)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    !ok
}

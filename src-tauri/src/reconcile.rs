use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::registry::RepoRegistry;
use crate::state::AppState;

#[derive(Debug, Default, Serialize)]
pub struct Discrepancies {
    pub orphaned_dirs: Vec<OrphanedDir>,
    pub missing_worktrees: Vec<MissingWorktree>,
}

#[derive(Debug, Serialize)]
pub struct OrphanedDir {
    pub path: PathBuf,
}

#[derive(Debug, Serialize)]
pub struct MissingWorktree {
    pub workspace_id: String,
    pub branch: String,
    pub repo_key: String,
    pub worktree_path: PathBuf,
}

pub async fn scan(
    state: &AppState,
    registry: Option<&RepoRegistry>,
    in_progress: &HashSet<String>,
) -> Discrepancies {
    let mut out = Discrepancies::default();

    for ws in &state.workspaces {
        for link in &ws.repo_links {
            if !link.worktree_path.exists() {
                out.missing_worktrees.push(MissingWorktree {
                    workspace_id: ws.id.clone(),
                    branch: ws.branch.clone(),
                    repo_key: link.repo_key.clone(),
                    worktree_path: link.worktree_path.clone(),
                });
            }
        }
    }

    let Some(reg) = registry else {
        return out;
    };
    // Read off stored paths: older workspace dirs are named by UUID, newer by
    // branch. Per-repo subdirs aren't checked; create never orphans just one.
    let mut known_dirs: HashSet<String> = HashSet::new();
    for ws in &state.workspaces {
        for link in &ws.repo_links {
            if let Some(name) = link
                .worktree_path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
            {
                known_dirs.insert(name.to_string());
            }
        }
    }

    let mut entries = match tokio::fs::read_dir(&reg.worktree_root).await {
        Ok(e) => e,
        Err(_) => return out,
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let Ok(ft) = entry.file_type().await else { continue };
        if !ft.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if known_dirs.contains(name) || in_progress.contains(name) {
            continue;
        }
        out.orphaned_dirs.push(OrphanedDir {
            path: path.clone(),
        });
    }

    out
}

pub fn is_under(worktree_root: &Path, candidate: &Path) -> bool {
    let Ok(root) = worktree_root.canonicalize() else {
        return false;
    };
    let Ok(cand) = candidate.canonicalize() else {
        return false;
    };
    cand.starts_with(&root) && cand != root
}

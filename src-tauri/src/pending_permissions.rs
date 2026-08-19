//! Captures permission entries that exist in a workspace's combined
//! `<workspace-root>/.claude/settings.local.json` but aren't present in the
//! union of its per-repo shared files. These come from sessions Claude
//! ran at the workspace root (cwd = workspace dir), where grants get
//! written to the combined file and would otherwise be orphaned on purge.
//!
//! The captured entries land in `<data_dir>/pending_permissions.json` and
//! are surfaced in the UI for the user to fold into the appropriate
//! per-repo file(s) or dismiss.

use std::collections::BTreeSet;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::fs;
use uuid::Uuid;

use crate::claude_settings::{PermissionCategory, PermissionEntry, SettingsDoc};
use crate::error::{AppError, AppResult};
use crate::paths::Paths;
use crate::state::Workspace;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingPermission {
    pub id: String,
    pub workspace_id: String,
    pub workspace_branch: String,
    /// Repos the source workspace contained at the moment of capture. Used
    /// by the UI to populate the "apply to which repo(s)" dropdown even
    /// after the workspace is gone.
    #[serde(default)]
    pub workspace_repo_keys: Vec<String>,
    pub captured_at: DateTime<Utc>,
    pub category: PermissionCategory,
    /// The entry as it appeared in the workspace-root combined file, with
    /// `./<repo-key>/...` prefixes preserved.
    pub raw_entry: String,
    /// When the entry's path argument starts with `./<repo-key>/` for one
    /// of `workspace_repo_keys`, the matched repo key. Used as the default
    /// target in the apply UI.
    pub suggested_repo_key: Option<String>,
    /// The entry with the matched `./<repo-key>/` prefix stripped — the
    /// form it should take in the per-repo shared file. Only set when
    /// `suggested_repo_key` is set.
    pub stripped_entry: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PendingPermissionsFile {
    #[serde(default)]
    pub entries: Vec<PendingPermission>,
}

/// Diff the workspace's combined settings file against the union of its
/// per-repo shared files and append any extra entries to the pending list.
/// Called from `purge_workspace` before the worktree dirs are removed.
pub async fn capture_for_purge(workspace: &Workspace, paths: &Paths) -> AppResult<()> {
    let Some(workspace_root) = workspace.root_buf() else {
        return Ok(());
    };

    let combined_path = workspace_root.join(".claude").join("settings.local.json");
    let combined = SettingsDoc::read(&combined_path).await?;

    let repo_keys: Vec<String> = workspace
        .repo_links
        .iter()
        .map(|r| r.repo_key.clone())
        .collect();

    // What the combined file should contain if nothing was granted inside a
    // workspace-root session: every per-repo entry, scoped to its repo.
    let mut expected: BTreeSet<String> = BTreeSet::new();
    for repo_key in &repo_keys {
        let repo_doc = SettingsDoc::read_lossy(&paths.repo_shared_claude_local(repo_key)).await;
        for category in PermissionCategory::ALL {
            for entry in repo_doc.permissions(category) {
                expected.insert(format!(
                    "{}:{}",
                    category.as_field(),
                    entry.scoped_to_repo(repo_key)
                ));
            }
        }
    }

    let now = Utc::now();
    let mut new_entries = Vec::new();
    for category in PermissionCategory::ALL {
        for entry in combined.permissions(category) {
            let raw = entry.to_string();
            if expected.contains(&format!("{}:{raw}", category.as_field())) {
                continue;
            }
            let unscoped = entry.unscope(&repo_keys);
            new_entries.push(PendingPermission {
                id: Uuid::new_v4().to_string(),
                workspace_id: workspace.id.clone(),
                workspace_branch: workspace.branch.clone(),
                workspace_repo_keys: repo_keys.clone(),
                captured_at: now,
                category,
                raw_entry: raw,
                suggested_repo_key: unscoped.as_ref().map(|(k, _)| k.clone()),
                stripped_entry: unscoped.map(|(_, e)| e.to_string()),
            });
        }
    }

    if new_entries.is_empty() {
        return Ok(());
    }

    let mut file = load_file(&paths.pending_permissions_file()).await?;
    file.entries.extend(new_entries);
    save_file(&paths.pending_permissions_file(), &file).await
}

pub async fn load_file(path: &Path) -> AppResult<PendingPermissionsFile> {
    match fs::read_to_string(path).await {
        Ok(s) => serde_json::from_str(&s)
            .map_err(|e| AppError::Other(format!("parsing pending_permissions.json: {e}"))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok(PendingPermissionsFile::default())
        }
        Err(e) => Err(AppError::Io(e)),
    }
}

async fn save_file(path: &Path, file: &PendingPermissionsFile) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }
    let content = serde_json::to_string_pretty(file)
        .map_err(|e| AppError::Other(format!("serializing pending_permissions.json: {e}")))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, content).await?;
    fs::rename(&tmp, path).await?;
    Ok(())
}

/// Apply a pending entry to one or more per-repo shared `settings.local.json`
/// files, then remove it from the pending list. Writing to a repo's shared
/// file means the entry persists into every workspace that includes that
/// repo from then on.
///
/// Uses `stripped_entry` only when applying to the suggested repo (where
/// the prefix corresponded). Otherwise writes `raw_entry` verbatim — the
/// caller is overriding our suggestion and is responsible for the form.
pub async fn apply_pending(
    paths: &Paths,
    pending_id: &str,
    target_repo_keys: &[String],
) -> AppResult<()> {
    let path = paths.pending_permissions_file();
    let mut file = load_file(&path).await?;

    let idx = file
        .entries
        .iter()
        .position(|e| e.id == pending_id)
        .ok_or_else(|| AppError::Other(format!("pending permission '{pending_id}' not found")))?;
    let entry = file.entries[idx].clone();

    for target in target_repo_keys {
        let to_write = if entry.suggested_repo_key.as_deref() == Some(target.as_str()) {
            entry.stripped_entry.clone().unwrap_or_else(|| entry.raw_entry.clone())
        } else {
            entry.raw_entry.clone()
        };
        write_into_per_repo_file(
            &paths.repo_shared_claude_local(target),
            entry.category,
            &to_write,
        )
        .await?;
    }

    file.entries.remove(idx);
    save_file(&path, &file).await
}

pub async fn dismiss_pending(paths: &Paths, pending_id: &str) -> AppResult<()> {
    let path = paths.pending_permissions_file();
    let mut file = load_file(&path).await?;
    let before = file.entries.len();
    file.entries.retain(|e| e.id != pending_id);
    if file.entries.len() == before {
        return Err(AppError::Other(format!(
            "pending permission '{pending_id}' not found"
        )));
    }
    save_file(&path, &file).await
}

async fn write_into_per_repo_file(
    path: &Path,
    category: PermissionCategory,
    entry_to_add: &str,
) -> AppResult<()> {
    let mut doc = SettingsDoc::read(path).await?;
    if !doc.add_permission(category, &PermissionEntry::parse(entry_to_add)) {
        return Ok(());
    }
    doc.write_atomic(path).await
}

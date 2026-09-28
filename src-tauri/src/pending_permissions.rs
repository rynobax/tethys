//! A session at the workspace root writes its grants to the root's
//! `.claude/settings.local.json`, which purge deletes. Anything there that no
//! per-repo shared file already has is kept for the user to fold into a repo
//! or dismiss.

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
    /// Outlives the workspace, for the apply UI's repo picker.
    #[serde(default)]
    pub workspace_repo_keys: Vec<String>,
    pub captured_at: DateTime<Utc>,
    pub category: PermissionCategory,
    /// As written at the root, `./<repo-key>/` prefixes included.
    pub raw_entry: String,
    /// The repo whose `./<repo-key>/` prefix the entry's path starts with.
    pub suggested_repo_key: Option<String>,
    /// `raw_entry` without that prefix, the form a per-repo file wants.
    pub stripped_entry: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PendingPermissionsFile {
    #[serde(default)]
    pub entries: Vec<PendingPermission>,
}

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

    // Every per-repo entry, scoped to its repo: what the root file holds if no
    // root session granted anything.
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

/// Any repo but the suggested one gets `raw_entry` verbatim: the prefix only
/// means something for the repo it names.
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

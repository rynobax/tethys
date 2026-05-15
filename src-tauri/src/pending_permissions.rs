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
use serde_json::{Map, Value};
use tokio::fs;
use tracing::warn;
use uuid::Uuid;

use crate::claude_local;
use crate::error::{AppError, AppResult};
use crate::paths::Paths;
use crate::state::Workspace;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionCategory {
    Allow,
    Deny,
    Ask,
}

impl PermissionCategory {
    fn as_field(&self) -> &'static str {
        match self {
            PermissionCategory::Allow => "allow",
            PermissionCategory::Deny => "deny",
            PermissionCategory::Ask => "ask",
        }
    }
}

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
    let Some(workspace_root) = workspace
        .repo_links
        .first()
        .and_then(|r| r.worktree_path.parent().map(|p| p.to_path_buf()))
    else {
        return Ok(());
    };

    let combined_path = workspace_root.join(".claude").join("settings.local.json");
    let combined: Value = match fs::read_to_string(&combined_path).await {
        Ok(s) => match serde_json::from_str(&s) {
            Ok(v) => v,
            Err(e) => {
                warn!(
                    error = %e,
                    path = %combined_path.display(),
                    "combined settings.local.json is not valid JSON; skipping pending capture"
                );
                return Ok(());
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(AppError::Io(e)),
    };

    let repo_keys: Vec<String> = workspace
        .repo_links
        .iter()
        .map(|r| r.repo_key.clone())
        .collect();
    let expected = expected_from_per_repo(&repo_keys, paths).await;

    let combined_perms = match combined.get("permissions").and_then(|v| v.as_object()) {
        Some(p) => p,
        None => return Ok(()),
    };

    let now = Utc::now();
    let mut new_entries = Vec::new();
    for category in [
        PermissionCategory::Allow,
        PermissionCategory::Deny,
        PermissionCategory::Ask,
    ] {
        let field = category.as_field();
        let Some(arr) = combined_perms.get(field).and_then(|v| v.as_array()) else {
            continue;
        };
        let expected_set = expected.set_for(category);
        for item in arr {
            let Some(s) = item.as_str() else { continue };
            if expected_set.contains(s) {
                continue;
            }
            let (suggested_repo_key, stripped_entry) = attribute(s, &repo_keys);
            new_entries.push(PendingPermission {
                id: Uuid::new_v4().to_string(),
                workspace_id: workspace.id.clone(),
                workspace_branch: workspace.branch.clone(),
                workspace_repo_keys: repo_keys.clone(),
                captured_at: now,
                category,
                raw_entry: s.to_string(),
                suggested_repo_key,
                stripped_entry,
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

struct Expected {
    allow: BTreeSet<String>,
    deny: BTreeSet<String>,
    ask: BTreeSet<String>,
}

impl Expected {
    fn set_for(&self, cat: PermissionCategory) -> &BTreeSet<String> {
        match cat {
            PermissionCategory::Allow => &self.allow,
            PermissionCategory::Deny => &self.deny,
            PermissionCategory::Ask => &self.ask,
        }
    }
}

async fn expected_from_per_repo(repo_keys: &[String], paths: &Paths) -> Expected {
    let mut allow = BTreeSet::new();
    let mut deny = BTreeSet::new();
    let mut ask = BTreeSet::new();

    for repo_key in repo_keys {
        let path = paths.repo_shared_claude_local(repo_key);
        let raw = match fs::read_to_string(&path).await {
            Ok(s) => s,
            Err(_) => continue,
        };
        let parsed: Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let Some(perms) = parsed.get("permissions").and_then(|v| v.as_object()) else {
            continue;
        };
        for (field, target) in [
            ("allow", &mut allow),
            ("deny", &mut deny),
            ("ask", &mut ask),
        ] {
            let Some(arr) = perms.get(field).and_then(|v| v.as_array()) else {
                continue;
            };
            for item in arr {
                let Some(s) = item.as_str() else { continue };
                target.insert(claude_local::rewrite_relative_path(s, repo_key));
            }
        }
    }

    Expected { allow, deny, ask }
}

/// Inverse of `rewrite_relative_path`: if `entry` has a path argument that
/// starts with `./<repo-key>/` for one of `repo_keys`, return the matched
/// key and the entry with the prefix stripped. Otherwise both are `None`.
fn attribute(entry: &str, repo_keys: &[String]) -> (Option<String>, Option<String>) {
    let Some(open) = entry.find('(') else {
        return (None, None);
    };
    let Some(close) = entry.rfind(')') else {
        return (None, None);
    };
    if close <= open + 1 {
        return (None, None);
    }
    let inside = &entry[open + 1..close];
    let Some(rest) = inside.strip_prefix("./") else {
        return (None, None);
    };
    for key in repo_keys {
        let prefix = format!("{key}/");
        if let Some(remainder) = rest.strip_prefix(&prefix) {
            let stripped = format!(
                "{}(./{}){}",
                &entry[..open],
                remainder,
                &entry[close + 1..]
            );
            return (Some(key.clone()), Some(stripped));
        }
    }
    (None, None)
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
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }
    let existing = match fs::read_to_string(path).await {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "{}".to_string(),
        Err(e) => return Err(AppError::Io(e)),
    };
    let mut root: Map<String, Value> = match serde_json::from_str::<Value>(&existing) {
        Ok(Value::Object(m)) => m,
        _ => Map::new(),
    };

    let permissions_entry = root
        .entry("permissions".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !permissions_entry.is_object() {
        *permissions_entry = Value::Object(Map::new());
    }
    let Value::Object(permissions_map) = permissions_entry else {
        unreachable!("just ensured object");
    };

    let field = category.as_field();
    let field_entry = permissions_map
        .entry(field.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    if !field_entry.is_array() {
        *field_entry = Value::Array(Vec::new());
    }
    let Value::Array(arr) = field_entry else {
        unreachable!("just ensured array");
    };

    if arr.iter().any(|v| v.as_str() == Some(entry_to_add)) {
        return Ok(());
    }
    arr.push(Value::String(entry_to_add.to_string()));

    let mut content = serde_json::to_string_pretty(&Value::Object(root))
        .map_err(|e| AppError::Other(format!("serializing per-repo settings.local.json: {e}")))?;
    content.push('\n');
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, content).await?;
    fs::rename(&tmp, path).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attribute_recognizes_repo_prefix() {
        let keys = vec!["api".to_string(), "frontend".to_string()];
        let (key, stripped) = attribute("Read(./api/src/foo.ts)", &keys);
        assert_eq!(key.as_deref(), Some("api"));
        assert_eq!(stripped.as_deref(), Some("Read(./src/foo.ts)"));
    }

    #[test]
    fn attribute_ignores_unknown_prefix() {
        let keys = vec!["api".to_string()];
        let (key, stripped) = attribute("Read(./other/src/foo.ts)", &keys);
        assert!(key.is_none());
        assert!(stripped.is_none());
    }

    #[test]
    fn attribute_ignores_non_path_entries() {
        let keys = vec!["api".to_string()];
        assert_eq!(attribute("Bash(rg:*)", &keys), (None, None));
        assert_eq!(attribute("mcp__linear__get_issue", &keys), (None, None));
        assert_eq!(attribute("Read(/abs/path)", &keys), (None, None));
    }
}

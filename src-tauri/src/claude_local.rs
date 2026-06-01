//! Shared `.claude/settings.local.json` per repo: one file under
//! `<data_dir>/symlinks/<repo-key>/settings.local.json` is symlinked into
//! every worktree Tethys creates for that repo, so permission edits in any
//! workspace propagate to all of them.
//!
//! For sessions started at the workspace *root* (parent of every repo's
//! worktree subdir), we seed a `<workspace-root>/.claude/settings.local.json`
//! at workspace-create time by union-merging each repo's permission lists.
//! After that initial seed, the file belongs to the workspace — Claude (or
//! you) may freely edit it. Tethys touches it again only to extend it with
//! a newly-added repo's entries, and on purge to diff its contents against
//! the per-repo files (so workspace-local grants can be merged back).

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::{Map, Value};
use tokio::fs;
use tracing::warn;

use crate::error::{AppError, AppResult};
use crate::job::JobTx;
use crate::paths::Paths;

/// Marker we write into the workspace-root settings.local.json so it's
/// identifiable as Tethys-seeded. Unlike before, the file is *not*
/// regenerated after seed — manual edits (and Claude's permission grants)
/// are preserved.
const SEEDED_MARKER: &str = "tethys (seeded on workspace create; safe to edit)";

/// Ensure `<worktree>/.claude/settings.local.json` is a symlink to the
/// repo's shared settings file, creating that file if it's the first
/// worktree to touch it. The shared file is (re-)seeded on every worktree
/// creation with a `sandbox.filesystem.allowWrite` grant for Tethys's repos
/// directory: every worktree's git metadata lives under there (in the app
/// data dir) rather than beside the worktree, so without this grant Claude
/// Code's sandbox denies routine git writes (index refresh, `add`,
/// `commit`). Claude Code unions these arrays across settings scopes, so the
/// grant coexists with whatever Claude later writes to the same file.
///
/// If the worktree already has a real file there (e.g. the repo tracks one),
/// leave it alone and warn — replacing it would show up as a git
/// modification and discard committed content.
pub async fn install_symlink(
    worktree_path: &Path,
    paths: &Paths,
    tx: &JobTx,
    repo_key: &str,
) -> AppResult<()> {
    let shared_path = paths.repo_shared_claude_local(repo_key);
    if let Some(parent) = shared_path.parent() {
        fs::create_dir_all(parent).await?;
    }
    let mut shared = read_root_object(&shared_path).await?;
    ensure_sandbox_allow_write(&mut shared, &paths.repos_clone_dir());
    write_json_object(&shared_path, &shared).await?;

    let claude_dir = worktree_path.join(".claude");
    fs::create_dir_all(&claude_dir).await?;
    let link_path = claude_dir.join("settings.local.json");

    match fs::symlink_metadata(&link_path).await {
        Ok(_) => {
            warn!(
                path = %link_path.display(),
                "settings.local.json already exists in worktree; skipping symlink"
            );
            tx.status(
                "settings.local.json already present; leaving as-is",
                Some(repo_key),
            );
            return Ok(());
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(AppError::Io(e)),
    }

    fs::symlink(&shared_path, &link_path).await?;
    tx.status(
        format!(
            "linked .claude/settings.local.json -> {}",
            shared_path.display()
        ),
        Some(repo_key),
    );
    Ok(())
}

/// Seed `<workspace_root>/.claude/settings.local.json` by union-merging
/// `permissions.allow` / `deny` / `ask` from each repo's shared
/// `settings.local.json`. File-glob entries that start with `./` are
/// rewritten to be relative to the workspace root (prefixed with the repo
/// key, which is also the worktree subdir name).
///
/// Called once at workspace create. After that, the file is owned by the
/// workspace (Claude may write to it, the user may edit it) and Tethys only
/// extends it via [`append_repo_to_workspace_root_settings`].
/// Missing or unparseable per-repo files are skipped with a warning.
pub async fn write_workspace_root_settings(
    workspace_root: &Path,
    repo_keys: &[String],
    paths: &Paths,
) -> AppResult<()> {
    if !fs::try_exists(workspace_root).await? {
        return Ok(());
    }

    /// Order-preserving deduped list. Insertion order is the merge order
    /// across repos, which the snapshot test relies on.
    #[derive(Default)]
    struct DedupList {
        items: Vec<String>,
        seen: BTreeSet<String>,
    }
    impl DedupList {
        fn push(&mut self, s: String) {
            if self.seen.insert(s.clone()) {
                self.items.push(s);
            }
        }
    }

    let mut allow = DedupList::default();
    let mut deny = DedupList::default();
    let mut ask = DedupList::default();

    for repo_key in repo_keys {
        let path = paths.repo_shared_claude_local(repo_key);
        let raw = match fs::read_to_string(&path).await {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                warn!(
                    error = %e,
                    path = %path.display(),
                    "failed to read repo shared settings.local.json"
                );
                continue;
            }
        };
        let parsed: Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => {
                warn!(
                    error = %e,
                    path = %path.display(),
                    "repo shared settings.local.json is not valid JSON"
                );
                continue;
            }
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
                target.push(rewrite_relative_path(s, repo_key));
            }
        }
    }

    let mut permissions = Map::new();
    for (field, list) in [("allow", allow), ("deny", deny), ("ask", ask)] {
        if !list.items.is_empty() {
            permissions.insert(
                field.into(),
                Value::Array(list.items.into_iter().map(Value::String).collect()),
            );
        }
    }

    let mut root = Map::new();
    root.insert("_seededBy".into(), Value::String(SEEDED_MARKER.into()));
    if !permissions.is_empty() {
        root.insert("permissions".into(), Value::Object(permissions));
    }

    // Sessions started at the workspace root run git against every repo's
    // worktree, whose git dirs live under the app data dir — outside the
    // sandbox's default writable set. Grant Tethys's repos dir (see
    // `install_symlink`).
    ensure_sandbox_allow_write(&mut root, &paths.repos_clone_dir());

    let claude_dir = workspace_root.join(".claude");
    fs::create_dir_all(&claude_dir).await?;
    let file_path = claude_dir.join("settings.local.json");
    write_json_object(&file_path, &root).await?;
    Ok(())
}

/// Extend an already-seeded `<workspace_root>/.claude/settings.local.json`
/// with the permission entries of a newly-added repo. Reads the existing
/// file, unions in the new repo's `allow` / `deny` / `ask` entries
/// (deduplicated against what's already there), and writes it back.
/// Any other keys / fields the user added to the file are preserved.
pub async fn append_repo_to_workspace_root_settings(
    workspace_root: &Path,
    repo_key: &str,
    paths: &Paths,
) -> AppResult<()> {
    if !fs::try_exists(workspace_root).await? {
        return Ok(());
    }
    let claude_dir = workspace_root.join(".claude");
    fs::create_dir_all(&claude_dir).await?;
    let file_path = claude_dir.join("settings.local.json");

    let mut root = read_root_object(&file_path).await?;

    let repo_settings_path = paths.repo_shared_claude_local(repo_key);
    let repo_perms = match fs::read_to_string(&repo_settings_path).await {
        Ok(s) => match serde_json::from_str::<Value>(&s) {
            Ok(v) => v,
            Err(e) => {
                warn!(
                    error = %e,
                    path = %repo_settings_path.display(),
                    "repo shared settings.local.json is not valid JSON"
                );
                Value::Null
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Value::Null,
        Err(e) => return Err(AppError::Io(e)),
    };

    if let Some(perms_obj) = repo_perms.get("permissions").and_then(|v| v.as_object()) {
        for field in ["allow", "deny", "ask"] {
            let Some(arr) = perms_obj.get(field).and_then(|v| v.as_array()) else {
                continue;
            };
            let items: Vec<String> = arr
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
            if items.is_empty() {
                continue;
            }
            merge_permission_field(&mut root, field, &items, repo_key);
        }
    }

    root.insert("_seededBy".into(), Value::String(SEEDED_MARKER.into()));
    root.remove("_generatedBy");

    write_json_object(&file_path, &root).await?;
    Ok(())
}

async fn read_root_object(path: &Path) -> AppResult<Map<String, Value>> {
    match fs::read_to_string(path).await {
        Ok(s) => match serde_json::from_str::<Value>(&s) {
            Ok(Value::Object(m)) => Ok(m),
            Ok(_) | Err(_) => {
                warn!(
                    path = %path.display(),
                    "existing settings.local.json was unparseable or not an object — replacing"
                );
                Ok(Map::new())
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Map::new()),
        Err(e) => Err(AppError::Io(e)),
    }
}

/// Ensure `root.sandbox.filesystem.allowWrite` contains `path`, creating the
/// nested `sandbox` / `filesystem` objects and the array as needed.
/// Deduplicates and leaves any sibling sandbox config untouched — Claude
/// Code deep-merges these objects across settings scopes, so we only
/// contribute our one entry.
fn ensure_sandbox_allow_write(root: &mut Map<String, Value>, path: &Path) {
    let path_str = path.to_string_lossy().into_owned();

    let sandbox = root
        .entry("sandbox".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !sandbox.is_object() {
        *sandbox = Value::Object(Map::new());
    }
    let Value::Object(sandbox_map) = sandbox else {
        return;
    };

    let filesystem = sandbox_map
        .entry("filesystem".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !filesystem.is_object() {
        *filesystem = Value::Object(Map::new());
    }
    let Value::Object(fs_map) = filesystem else {
        return;
    };

    let allow = fs_map
        .entry("allowWrite".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    if !allow.is_array() {
        *allow = Value::Array(Vec::new());
    }
    let Value::Array(arr) = allow else {
        return;
    };

    if !arr.iter().any(|v| v.as_str() == Some(path_str.as_str())) {
        arr.push(Value::String(path_str));
    }
}

/// Serialize a settings object to pretty JSON (with a trailing newline) and
/// write it to `path`.
async fn write_json_object(path: &Path, root: &Map<String, Value>) -> AppResult<()> {
    let mut content = serde_json::to_string_pretty(root)
        .map_err(|e| AppError::Other(format!("serializing settings.local.json: {e}")))?;
    content.push('\n');
    fs::write(path, content).await?;
    Ok(())
}

fn merge_permission_field(
    root: &mut Map<String, Value>,
    field: &str,
    items: &[String],
    repo_key: &str,
) {
    let permissions_entry = root
        .entry("permissions".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !permissions_entry.is_object() {
        *permissions_entry = Value::Object(Map::new());
    }
    let Value::Object(permissions_map) = permissions_entry else {
        return;
    };

    let field_entry = permissions_map
        .entry(field.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    if !field_entry.is_array() {
        *field_entry = Value::Array(Vec::new());
    }
    let Value::Array(arr) = field_entry else {
        return;
    };

    let mut seen: BTreeSet<String> = arr
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    for item in items {
        let rewritten = rewrite_relative_path(item, repo_key);
        if seen.insert(rewritten.clone()) {
            arr.push(Value::String(rewritten));
        }
    }
}

/// Rewrite a permission entry's path to be relative to the workspace root
/// instead of the repo worktree. Only touches entries whose argument starts
/// with `./` (e.g. `Read(./src/**)` → `Read(./<repo_key>/src/**)`); leaves
/// `Bash(...)`, `WebFetch(domain:...)`, absolute paths, `~/...`, and
/// argument-less entries (`mcp__...`, `Skill(...)`) untouched.
pub(crate) fn rewrite_relative_path(entry: &str, repo_key: &str) -> String {
    let Some(open) = entry.find('(') else {
        return entry.to_string();
    };
    let Some(close) = entry.rfind(')') else {
        return entry.to_string();
    };
    if close <= open + 1 {
        return entry.to_string();
    }
    let inside = &entry[open + 1..close];
    let Some(rest) = inside.strip_prefix("./") else {
        return entry.to_string();
    };
    format!(
        "{}(./{}/{}){}",
        &entry[..open],
        repo_key,
        rest,
        &entry[close + 1..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrite_relative_only_touches_dot_slash() {
        assert_eq!(
            rewrite_relative_path("Read(./src/**)", "frontend"),
            "Read(./frontend/src/**)"
        );
        assert_eq!(
            rewrite_relative_path("Bash(yarn test:*)", "frontend"),
            "Bash(yarn test:*)"
        );
        assert_eq!(
            rewrite_relative_path("WebFetch(domain:github.com)", "frontend"),
            "WebFetch(domain:github.com)"
        );
        assert_eq!(
            rewrite_relative_path("Read(//Users/ryan/x/**)", "frontend"),
            "Read(//Users/ryan/x/**)"
        );
        assert_eq!(
            rewrite_relative_path("Read(~/Downloads/**)", "frontend"),
            "Read(~/Downloads/**)"
        );
        assert_eq!(
            rewrite_relative_path("mcp__linear__get_issue", "frontend"),
            "mcp__linear__get_issue"
        );
        assert_eq!(
            rewrite_relative_path("Skill(see-data)", "frontend"),
            "Skill(see-data)"
        );
    }

    #[tokio::test]
    async fn merges_dedupes_and_writes() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().to_path_buf();
        let paths = Paths {
            data_dir: data_dir.clone(),
        };

        let frontend_settings = paths.repo_shared_claude_local("frontend");
        let backend_settings = paths.repo_shared_claude_local("backend");
        fs::create_dir_all(frontend_settings.parent().unwrap())
            .await
            .unwrap();
        fs::create_dir_all(backend_settings.parent().unwrap())
            .await
            .unwrap();
        fs::write(
            &frontend_settings,
            r#"{"permissions":{"allow":["Bash(grep:*)","Read(./src/**)"],"deny":["Bash(rm:*)"]}}"#,
        )
        .await
        .unwrap();
        fs::write(
            &backend_settings,
            r#"{"permissions":{"allow":["Bash(grep:*)","Bash(pytest:*)"]}}"#,
        )
        .await
        .unwrap();

        let workspace_root = data_dir.join("ws");
        fs::create_dir_all(&workspace_root).await.unwrap();
        write_workspace_root_settings(
            &workspace_root,
            &["frontend".into(), "backend".into()],
            &paths,
        )
        .await
        .unwrap();

        let written =
            fs::read_to_string(workspace_root.join(".claude/settings.local.json"))
                .await
                .unwrap();
        let parsed: Value = serde_json::from_str(&written).unwrap();
        let allow = parsed["permissions"]["allow"].as_array().unwrap();
        let allow_strs: Vec<&str> = allow.iter().filter_map(|v| v.as_str()).collect();
        assert_eq!(
            allow_strs,
            vec![
                "Bash(grep:*)",
                "Read(./frontend/src/**)",
                "Bash(pytest:*)",
            ],
            "dedupes Bash(grep:*) across repos and rewrites ./ paths"
        );
        let deny = parsed["permissions"]["deny"].as_array().unwrap();
        assert_eq!(deny.len(), 1);
        assert_eq!(deny[0].as_str(), Some("Bash(rm:*)"));
        assert!(!parsed["_seededBy"].as_str().unwrap_or("").is_empty());

        let allow_write = parsed["sandbox"]["filesystem"]["allowWrite"]
            .as_array()
            .unwrap();
        assert_eq!(
            allow_write.len(),
            1,
            "a single constant grant covers every repo's git data"
        );
        assert_eq!(
            allow_write[0].as_str(),
            Some(paths.repos_clone_dir().to_string_lossy().as_ref()),
            "Tethys's repos dir is granted to the sandbox"
        );
    }

    #[tokio::test]
    async fn skips_when_workspace_root_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths {
            data_dir: tmp.path().to_path_buf(),
        };
        let missing = tmp.path().join("does-not-exist");
        write_workspace_root_settings(&missing, &["any".into()], &paths)
            .await
            .unwrap();
        assert!(!missing.exists());
    }

    #[tokio::test]
    async fn missing_per_repo_files_are_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths {
            data_dir: tmp.path().to_path_buf(),
        };
        let workspace_root = tmp.path().join("ws");
        fs::create_dir_all(&workspace_root).await.unwrap();
        write_workspace_root_settings(
            &workspace_root,
            &["never-symlinked".into()],
            &paths,
        )
        .await
        .unwrap();
        let written =
            fs::read_to_string(workspace_root.join(".claude/settings.local.json"))
                .await
                .unwrap();
        let parsed: Value = serde_json::from_str(&written).unwrap();
        // No permissions block when nothing was found.
        assert!(parsed.get("permissions").is_none());
        // The sandbox grant is still added — it's derived from the repo key,
        // not from the (missing) per-repo settings file.
        let allow_write = parsed["sandbox"]["filesystem"]["allowWrite"]
            .as_array()
            .unwrap();
        assert_eq!(allow_write.len(), 1);
    }

    #[test]
    fn ensure_sandbox_allow_write_dedupes_and_preserves_siblings() {
        let mut root = serde_json::json!({
            "sandbox": {
                "network": { "allowedDomains": ["github.com"] },
                "filesystem": { "allowWrite": ["/already/here"] }
            }
        })
        .as_object()
        .unwrap()
        .clone();

        let git_dir = Path::new("/data/repos/x/.git");
        ensure_sandbox_allow_write(&mut root, git_dir);
        // Second call must be a no-op (deduped).
        ensure_sandbox_allow_write(&mut root, git_dir);

        let value = Value::Object(root);
        let allow = value["sandbox"]["filesystem"]["allowWrite"]
            .as_array()
            .unwrap();
        assert_eq!(allow.len(), 2);
        assert_eq!(allow[0].as_str(), Some("/already/here"));
        assert_eq!(allow[1].as_str(), Some("/data/repos/x/.git"));
        // Unrelated sandbox config survives the deep insert.
        assert_eq!(
            value["sandbox"]["network"]["allowedDomains"][0].as_str(),
            Some("github.com")
        );
    }

    #[tokio::test]
    async fn install_symlink_seeds_sandbox_git_dir_grant() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths {
            data_dir: tmp.path().to_path_buf(),
        };
        let worktree = tmp.path().join("wt");
        fs::create_dir_all(&worktree).await.unwrap();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let job_tx = JobTx(tx);

        install_symlink(&worktree, &paths, &job_tx, "backend")
            .await
            .unwrap();

        let shared = paths.repo_shared_claude_local("backend");
        let parsed: Value =
            serde_json::from_str(&fs::read_to_string(&shared).await.unwrap()).unwrap();
        let allow_write = parsed["sandbox"]["filesystem"]["allowWrite"]
            .as_array()
            .unwrap();
        assert_eq!(allow_write.len(), 1);
        assert_eq!(
            allow_write[0].as_str(),
            Some(paths.repos_clone_dir().to_string_lossy().as_ref())
        );

        // The worktree's settings.local.json is a symlink to the shared file.
        let link = worktree.join(".claude/settings.local.json");
        assert_eq!(fs::read_link(&link).await.unwrap(), shared);
    }
}

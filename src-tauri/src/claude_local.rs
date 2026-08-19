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

use std::path::Path;

use serde_json::Value;
use tokio::fs;
use tracing::warn;

use crate::claude_settings::{PermissionCategory, SettingsDoc};

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
/// creation with a `sandbox.filesystem.allowWrite` grant for the repo's clone
/// `.git` directory: every worktree's git metadata lives under there (in the
/// app data dir) rather than beside the worktree, so without this grant Claude
/// Code's sandbox denies routine git writes (index refresh, `add`, `commit`,
/// branch tracking, push). Scoping to `.git` keeps the clone's source tree
/// read-only. Claude Code unions these arrays across settings scopes, so the
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
    let mut shared = SettingsDoc::read(&shared_path).await?;
    // Migrate files seeded by the earlier fix that granted the whole repos
    // dir (which left the clone's source tree writable); the scoped `.git`
    // grant below replaces it.
    shared.revoke_write(&paths.repos_clone_dir());
    shared.allow_write(&paths.repo_git_dir(repo_key));
    shared.write_atomic(&shared_path).await?;

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

    let mut root = SettingsDoc::new();
    root.set("_seededBy", Value::String(SEEDED_MARKER.into()));

    // Merge each repo's shared entries in, rewritten to be relative to the
    // workspace root. Order is repo order then file order, which the snapshot
    // test relies on; `add_permission` dedupes.
    for repo_key in repo_keys {
        let repo_doc = SettingsDoc::read_lossy(&paths.repo_shared_claude_local(repo_key)).await;
        for category in PermissionCategory::ALL {
            for entry in repo_doc.permissions(category) {
                root.add_permission(category, &entry.scoped_to_repo(repo_key));
            }
        }
    }

    // Sessions started at the workspace root run git against every repo's
    // worktree, whose git dirs live under the app data dir — outside the
    // sandbox's default writable set. Grant each repo's `.git` (see
    // `install_symlink`).
    for repo_key in repo_keys {
        root.allow_write(&paths.repo_git_dir(repo_key));
    }

    let file_path = workspace_root.join(".claude").join("settings.local.json");
    root.write_atomic(&file_path).await?;
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
    let file_path = workspace_root.join(".claude").join("settings.local.json");
    let mut root = SettingsDoc::read(&file_path).await?;

    let repo_doc = SettingsDoc::read_lossy(&paths.repo_shared_claude_local(repo_key)).await;
    for category in PermissionCategory::ALL {
        for entry in repo_doc.permissions(category) {
            root.add_permission(category, &entry.scoped_to_repo(repo_key));
        }
    }

    root.set("_seededBy", Value::String(SEEDED_MARKER.into()));
    root.remove("_generatedBy");

    root.write_atomic(&file_path).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;


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
        let allow_write_strs: Vec<&str> =
            allow_write.iter().filter_map(|v| v.as_str()).collect();
        assert_eq!(
            allow_write_strs,
            vec![
                paths.repo_git_dir("frontend").to_string_lossy().as_ref(),
                paths.repo_git_dir("backend").to_string_lossy().as_ref(),
            ],
            "each repo's clone .git dir is granted — not the source-bearing repos dir"
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
        assert_eq!(
            allow_write[0].as_str(),
            Some(paths.repo_git_dir("never-symlinked").to_string_lossy().as_ref())
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
            Some(paths.repo_git_dir("backend").to_string_lossy().as_ref())
        );

        // The worktree's settings.local.json is a symlink to the shared file.
        let link = worktree.join(".claude/settings.local.json");
        assert_eq!(fs::read_link(&link).await.unwrap(), shared);
    }

    #[tokio::test]
    async fn install_symlink_retires_old_repos_dir_grant() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths {
            data_dir: tmp.path().to_path_buf(),
        };
        // Pre-seed the shared file with the old over-broad grant.
        let shared = paths.repo_shared_claude_local("backend");
        fs::create_dir_all(shared.parent().unwrap()).await.unwrap();
        let mut root = SettingsDoc::new();
        root.allow_write(&paths.repos_clone_dir());
        root.write_atomic(&shared).await.unwrap();

        let worktree = tmp.path().join("wt");
        fs::create_dir_all(&worktree).await.unwrap();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        install_symlink(&worktree, &paths, &JobTx(tx), "backend")
            .await
            .unwrap();

        let parsed: Value =
            serde_json::from_str(&fs::read_to_string(&shared).await.unwrap()).unwrap();
        let allow_write: Vec<&str> = parsed["sandbox"]["filesystem"]["allowWrite"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(
            allow_write,
            vec![paths.repo_git_dir("backend").to_string_lossy().as_ref()],
            "the broad repos-dir grant is replaced by the scoped .git grant"
        );
    }
}

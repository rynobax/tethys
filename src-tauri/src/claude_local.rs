//! Each repo has one shared `settings.local.json`, symlinked into every
//! worktree of it so a permission granted in one workspace applies to all.
//!
//! The workspace root gets its own, seeded from the union of its repos' files.
//! After seeding it's the workspace's to edit; Tethys only ever extends it.

use std::path::Path;

use serde_json::Value;
use tokio::fs;
use tracing::warn;

use crate::claude_settings::{PermissionCategory, SettingsDoc};

use crate::error::{AppError, AppResult};
use crate::job::JobTx;
use crate::paths::Paths;

const SEEDED_MARKER: &str = "tethys (seeded on workspace create; safe to edit)";

/// A real file already at the link path (one the repo tracks) is left alone:
/// replacing it would discard committed content.
pub async fn install_symlink(
    worktree_path: &Path,
    paths: &Paths,
    tx: &JobTx,
    repo_key: &str,
) -> AppResult<()> {
    let shared_path = paths.repo_shared_claude_local(repo_key);
    let mut shared = SettingsDoc::read(&shared_path).await?;
    // Retire an older grant of the whole repos dir, which left clone sources
    // writable.
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

    for repo_key in repo_keys {
        let repo_doc = SettingsDoc::read_lossy(&paths.repo_shared_claude_local(repo_key)).await;
        for category in PermissionCategory::ALL {
            for entry in repo_doc.permissions(category) {
                root.add_permission(category, &entry.scoped_to_repo(repo_key));
            }
        }
    }

    for repo_key in repo_keys {
        root.allow_write(&paths.repo_git_dir(repo_key));
    }

    let file_path = workspace_root.join(".claude").join("settings.local.json");
    root.write_atomic(&file_path).await?;
    Ok(())
}

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
        assert!(parsed.get("permissions").is_none());
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

        let link = worktree.join(".claude/settings.local.json");
        assert_eq!(fs::read_link(&link).await.unwrap(), shared);
    }

    #[tokio::test]
    async fn install_symlink_retires_old_repos_dir_grant() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths {
            data_dir: tmp.path().to_path_buf(),
        };
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

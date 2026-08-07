use std::path::PathBuf;
use tauri::{AppHandle, Manager};

use crate::error::{AppError, AppResult};

#[derive(Clone)]
pub struct Paths {
    pub data_dir: PathBuf,
}

impl Paths {
    pub fn from_app(app: &AppHandle) -> AppResult<Self> {
        let data_dir = app
            .path()
            .app_data_dir()
            .map_err(|e| AppError::Other(format!("resolving app data dir: {e}")))?;
        std::fs::create_dir_all(&data_dir)?;
        std::fs::create_dir_all(data_dir.join("logs"))?;
        Ok(Self { data_dir })
    }

    pub fn state_file(&self) -> PathBuf {
        self.data_dir.join("state.json")
    }

    pub fn state_tmp_file(&self) -> PathBuf {
        self.data_dir.join("state.json.tmp")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.data_dir.join("logs")
    }

    pub fn repos_config_file(&self) -> PathBuf {
        self.data_dir.join("repos.toml")
    }

    pub fn repos_schema_file(&self) -> PathBuf {
        self.data_dir.join("repos.schema.json")
    }

    pub fn repos_clone_dir(&self) -> PathBuf {
        self.data_dir.join("repos")
    }

    pub fn repo_clone_path(&self, repo_key: &str) -> PathBuf {
        self.repos_clone_dir().join(repo_key)
    }

    /// The clone's git directory — `.../repos/<repo_key>/.git`. Every worktree
    /// Tethys branches off this clone keeps its git metadata here (shared
    /// `config`/`refs`/`logs` plus a per-worktree subdir under
    /// `.git/worktrees/`), so this is the path the sandbox must let git write.
    /// Scoped to `.git` specifically so the clone's source tree beside it
    /// stays read-only.
    pub fn repo_git_dir(&self, repo_key: &str) -> PathBuf {
        self.repo_clone_path(repo_key).join(".git")
    }

    pub fn symlinks_dir(&self) -> PathBuf {
        self.data_dir.join("symlinks")
    }

    /// Shared `settings.local.json` for a repo — symlinked into each of that
    /// repo's worktrees so permissions stay in sync across workspaces.
    pub fn repo_shared_claude_local(&self, repo_key: &str) -> PathBuf {
        self.symlinks_dir().join(repo_key).join("settings.local.json")
    }

    pub fn hook_socket(&self) -> PathBuf {
        self.data_dir.join("hook.sock")
    }

    pub fn claude_settings_lock(&self) -> PathBuf {
        self.data_dir.join("claude-settings.lock")
    }

    pub fn theme_file(&self) -> PathBuf {
        self.data_dir.join("theme.json")
    }

    /// JSON file of permission entries captured on workspace purge, waiting
    /// for the user to review and either fold into per-repo shared
    /// `settings.local.json` files or discard.
    pub fn pending_permissions_file(&self) -> PathBuf {
        self.data_dir.join("pending_permissions.json")
    }
}

/// `~/.claude/settings.json` — user-level Claude Code settings.
pub fn claude_settings_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".claude").join("settings.json"))
}

/// Resolve the tethys-hook companion binary next to the current executable.
/// In dev, Cargo places both at `<workspace>/target/debug/`; in a bundled
/// app they'd need to sit side by side too.
pub fn tethys_hook_bin() -> std::io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let parent = exe.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "no parent for current exe")
    })?;
    Ok(parent.join("tethys-hook"))
}

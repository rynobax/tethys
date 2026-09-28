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

    /// Every worktree's git metadata lives here, so it's what a sandbox must
    /// let git write, while the clone's source tree beside it stays read-only.
    pub fn repo_git_dir(&self, repo_key: &str) -> PathBuf {
        self.repo_clone_path(repo_key).join(".git")
    }

    pub fn symlinks_dir(&self) -> PathBuf {
        self.data_dir.join("symlinks")
    }

    pub fn repo_shared_claude_local(&self, repo_key: &str) -> PathBuf {
        self.symlinks_dir().join(repo_key).join("settings.local.json")
    }

    pub fn hook_socket(&self) -> PathBuf {
        self.data_dir.join("hook.sock")
    }

    /// Separate from `hook.sock`: MCP is request/reply and its failures must
    /// surface, where hooks are fire-and-forget.
    pub fn mcp_socket(&self) -> PathBuf {
        self.data_dir.join("mcp.sock")
    }

    pub fn claude_settings_lock(&self) -> PathBuf {
        self.data_dir.join("claude-settings.lock")
    }

    pub fn codex_config_lock(&self) -> PathBuf {
        self.data_dir.join("codex-config.lock")
    }

    pub fn theme_file(&self) -> PathBuf {
        self.data_dir.join("theme.json")
    }

    pub fn pending_permissions_file(&self) -> PathBuf {
        self.data_dir.join("pending_permissions.json")
    }
}

pub fn claude_settings_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".claude").join("settings.json"))
}

/// `$CODEX_HOME` wins, as it does for codex itself.
pub fn codex_config_path() -> Option<PathBuf> {
    let dir = match std::env::var_os("CODEX_HOME") {
        Some(home) => PathBuf::from(home),
        None => PathBuf::from(std::env::var_os("HOME")?).join(".codex"),
    };
    Some(dir.join("config.toml"))
}

fn companion_bin(name: &str) -> std::io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let parent = exe.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "no parent for current exe")
    })?;
    Ok(parent.join(name))
}

pub fn tethys_hook_bin() -> std::io::Result<PathBuf> {
    companion_bin("tethys-hook")
}

pub fn tethys_mcp_bin() -> std::io::Result<PathBuf> {
    companion_bin("tethys-mcp")
}

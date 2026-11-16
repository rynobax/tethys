use std::path::PathBuf;

use crate::error::{AppError, AppResult};

/// Also read by `tethys-hook`, which must find the same `hook.sock`.
pub const DATA_DIR_VAR: &str = "TETHYS_DATA_DIR";

#[derive(Clone)]
pub struct Paths {
    pub data_dir: PathBuf,
    /// Set by `TETHYS_DATA_DIR`: a second Tethys beside the real one, which
    /// must leave the real one's tmux server, processes and config alone.
    pub isolated: bool,
}

impl Paths {
    /// Unset, the data dir is where Tauri put it, so existing state carries over.
    pub fn resolve() -> AppResult<Self> {
        let (data_dir, isolated) = match std::env::var_os(DATA_DIR_VAR).filter(|v| !v.is_empty()) {
            Some(dir) => (PathBuf::from(dir), true),
            None => {
                let home = std::env::var_os("HOME")
                    .ok_or_else(|| AppError::Other("HOME not set".into()))?;
                let dir = PathBuf::from(home)
                    .join("Library")
                    .join("Application Support")
                    .join("app.tethys.dev");
                (dir, false)
            }
        };
        std::fs::create_dir_all(&data_dir)?;
        // Sessions run elsewhere and reach the sockets by this path.
        let data_dir = std::fs::canonicalize(&data_dir)?;
        Ok(Self { data_dir, isolated })
    }

    /// `None` for the real server. Stable per data dir, so an isolated run
    /// reattaches its own sessions on restart.
    pub fn isolated_tmux_label(&self) -> Option<String> {
        if !self.isolated {
            return None;
        }
        // FNV-1a: std's hasher isn't promised stable across releases.
        let hash = self
            .data_dir
            .as_os_str()
            .as_encoded_bytes()
            .iter()
            .fold(0xcbf29ce484222325u64, |h, b| {
                (h ^ u64::from(*b)).wrapping_mul(0x100000001b3)
            });
        Some(format!("tethys-iso-{:08x}", hash as u32))
    }

    pub fn ui_socket(&self) -> PathBuf {
        self.data_dir.join("ui.sock")
    }

    pub fn state_file(&self) -> PathBuf {
        self.data_dir.join("state.json")
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

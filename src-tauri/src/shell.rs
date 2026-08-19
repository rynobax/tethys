use std::path::PathBuf;
use std::process::Command;

use tracing::{info, warn};

use crate::error::{AppError, AppResult};

/// Resolve `bin` to an absolute path by asking a login shell
/// (`/bin/zsh -ilc 'which <bin>'`).
///
/// Desktop apps on macOS inherit a minimal `$PATH` — no nvm, volta, or
/// Homebrew dirs — so every binary Tethys shells out to has to be found the
/// way the user's own terminal would find it.
///
/// `install_hint` is the shell command to suggest when the binary is missing
/// (e.g. `"brew install tmux"`); `None` gives generic advice.
pub fn which(bin: &str, install_hint: Option<&str>) -> AppResult<PathBuf> {
    if bin.is_empty() || bin.contains(|c: char| c.is_whitespace() || c == '\'' || c == '"') {
        return Err(AppError::Other(format!("invalid binary name: {bin:?}")));
    }
    let cmd = format!("which {bin}");
    let output = Command::new("/bin/zsh")
        .args(["-ilc", &cmd])
        .output()
        .map_err(|e| AppError::Other(format!("failed to invoke /bin/zsh: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(AppError::Other(format!(
            "`which {bin}` via /bin/zsh failed: {}",
            if stderr.is_empty() { "no stderr" } else { &stderr }
        )));
    }

    let raw = String::from_utf8_lossy(&output.stdout).to_string();
    let path = extract_path(&raw);

    if path.is_empty() || !path.starts_with('/') {
        warn!(?path, %bin, "binary not on login-shell PATH");
        let how = match install_hint {
            Some(hint) => format!("install with `{hint}`"),
            None => "install it".to_string(),
        };
        return Err(AppError::Other(format!(
            "{bin} not found — {how} and make sure `which {bin}` works in a login shell"
        )));
    }

    info!(%path, %bin, "resolved binary via login shell");
    Ok(PathBuf::from(path))
}

/// Pull the actual command output from `which <bin>` after shell-integration
/// noise. iTerm2 + zsh interactive mode prepends OSC escapes (ending in BEL
/// `\x07`) before stdout gets piped to us — everything before the final BEL
/// is preamble, not the path we want.
pub fn extract_path(raw: &str) -> String {
    let trimmed = match raw.rfind('\x07') {
        Some(idx) => &raw[idx + 1..],
        None => raw,
    };
    trimmed.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::extract_path;

    #[test]
    fn plain_output() {
        assert_eq!(extract_path("/usr/local/bin/claude\n"), "/usr/local/bin/claude");
    }

    #[test]
    fn iterm_osc_prefix() {
        let raw = "\x1b]1337;RemoteHost=ryan@host\x07\x1b]1337;CurrentDir=/cwd\x07/Users/ryan/.local/bin/claude\n";
        assert_eq!(extract_path(raw), "/Users/ryan/.local/bin/claude");
    }

    /// Name validation runs before we ever spawn a shell, so these are the
    /// one part of `which` that is testable without a login shell.
    #[test]
    fn rejects_names_that_could_break_out_of_the_which_command() {
        for bad in ["", "cla ude", "claude'", "claude\"", "claude; rm -rf /"] {
            let err = super::which(bad, None).unwrap_err().to_string();
            assert!(err.contains("invalid binary name"), "{bad:?} -> {err}");
        }
    }

    #[test]
    fn no_bell_returns_trimmed_input() {
        assert_eq!(extract_path("  /opt/homebrew/bin/tmux  \n"), "/opt/homebrew/bin/tmux");
    }
}

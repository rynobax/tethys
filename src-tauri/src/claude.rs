use std::path::PathBuf;

use crate::error::AppResult;
use crate::shell;

/// Resolve the absolute path to the `claude` binary by running
/// `/bin/zsh -ilc 'which claude'`. Desktop apps on macOS inherit a minimal
/// `$PATH` (no nvm/volta/homebrew dirs), so this is how we reliably find
/// whatever the user has on their login shell `PATH`.
///
/// Called once at boot and cached; re-resolve manually if the user moves
/// their install.
pub fn resolve() -> AppResult<PathBuf> {
    resolve_named("claude")
}

/// Like `resolve` but for an arbitrary entry-point name (e.g. `claude-hipaa`),
/// so per-workspace binary overrides can use the same login-shell PATH lookup.
pub fn resolve_named(bin: &str) -> AppResult<PathBuf> {
    shell::which(bin, None)
}

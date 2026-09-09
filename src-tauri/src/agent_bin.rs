//! Finding an agent CLI on disk.

use std::path::{Path, PathBuf};

use crate::agent::Agent;
use crate::error::AppResult;
use crate::shell;

/// Resolve the absolute path to an agent's default binary by running
/// `/bin/zsh -ilc 'which <bin>'`. Desktop apps on macOS inherit a minimal
/// `$PATH` (no nvm/volta/homebrew dirs), so this is how we reliably find
/// whatever the user has on their login shell `PATH`.
///
/// Called once per agent at boot and cached; re-resolve manually if the user
/// moves their install.
pub fn resolve(agent: Agent) -> AppResult<PathBuf> {
    resolve_named(agent.default_binary())
}

/// Like `resolve` but for an arbitrary entry-point name (e.g. `claude-hipaa`),
/// so per-workspace binary overrides can use the same login-shell PATH lookup.
pub fn resolve_named(bin: &str) -> AppResult<PathBuf> {
    shell::which(bin, None)
}

/// The default binary for each agent, resolved once at boot and managed in
/// Tauri state.
///
/// An agent that isn't installed holds an empty path rather than being absent:
/// the failure belongs at spawn time, where it can be reported against the
/// workspace the user just tried to start, not at boot where it would be a
/// warning nobody reads.
#[derive(Clone)]
pub struct AgentBins {
    claude: PathBuf,
    codex: PathBuf,
}

impl AgentBins {
    /// Resolve every agent's default binary. Never fails — an agent that isn't
    /// on the login-shell PATH gets an empty path and the warning is logged.
    pub fn resolve_all() -> Self {
        Self {
            claude: resolve_or_warn(Agent::Claude),
            codex: resolve_or_warn(Agent::Codex),
        }
    }

    pub fn get(&self, agent: Agent) -> &Path {
        match agent {
            Agent::Claude => &self.claude,
            Agent::Codex => &self.codex,
        }
    }
}

fn resolve_or_warn(agent: Agent) -> PathBuf {
    match resolve(agent) {
        Ok(path) => path,
        Err(e) => {
            tracing::warn!(
                agent = agent.default_binary(),
                error = %e,
                "agent binary not resolved at startup"
            );
            PathBuf::new()
        }
    }
}

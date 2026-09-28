use std::path::{Path, PathBuf};

use crate::agent::Agent;
use crate::error::AppResult;
use crate::shell;

pub fn resolve(agent: Agent) -> AppResult<PathBuf> {
    resolve_named(agent.default_binary())
}

pub fn resolve_named(bin: &str) -> AppResult<PathBuf> {
    shell::which(bin, None)
}

/// A missing agent holds an empty path so the failure surfaces at spawn,
/// against the workspace being started, rather than as an unread boot warning.
#[derive(Clone)]
pub struct AgentBins {
    claude: PathBuf,
    codex: PathBuf,
}

impl AgentBins {
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

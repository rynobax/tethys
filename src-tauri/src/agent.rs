//! Which agent CLI a workspace's session runs.
//!
//! Two harnesses, named outright rather than described by a trait. They differ
//! too deeply to factor cleanly — Claude Code registers hooks in a settings
//! file and resumes with a flag; codex takes both on its command line, and
//! nothing Tethys does with one is a special case of what it does with the
//! other. A `match` at each of the handful of sites that care is honest about
//! that; an abstraction built against a sample of two would not be.
//!
//! The kind is stored, never derived from the binary name. `claude-hipaa` is a
//! Claude; a wrapper script called `cx` would be a codex; and the kind decides
//! things — hook installation, resume syntax, MCP wiring — that must not hinge
//! on how an executable happens to be spelled.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Agent {
    /// Anthropic's Claude Code CLI. The default, and what every workspace
    /// persisted before codex support was one of.
    #[default]
    Claude,
    /// OpenAI's codex CLI.
    Codex,
}

impl Agent {
    /// The binary this agent runs as when the workspace names no override.
    pub fn default_binary(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
        }
    }

    /// How to say this agent in a sentence to the user.
    pub fn label(self) -> &'static str {
        match self {
            Agent::Claude => "Claude",
            Agent::Codex => "codex",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// State written before the field existed has to load as Claude — that is
    /// what every one of those workspaces was.
    #[test]
    fn claude_is_the_default() {
        assert_eq!(Agent::default(), Agent::Claude);
    }

    #[test]
    fn round_trips_as_snake_case() {
        for agent in [Agent::Claude, Agent::Codex] {
            let json = serde_json::to_string(&agent).unwrap();
            assert_eq!(serde_json::from_str::<Agent>(&json).unwrap(), agent);
        }
        assert_eq!(serde_json::to_string(&Agent::Codex).unwrap(), "\"codex\"");
    }
}

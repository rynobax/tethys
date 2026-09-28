//! Stored, never derived from the binary name: `claude-hipaa` is a Claude.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Agent {
    #[default]
    Claude,
    Codex,
}

impl Agent {
    pub fn default_binary(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
        }
    }

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

    #[test]
    fn round_trips_as_snake_case() {
        for agent in [Agent::Claude, Agent::Codex] {
            let json = serde_json::to_string(&agent).unwrap();
            assert_eq!(serde_json::from_str::<Agent>(&json).unwrap(), agent);
        }
        assert_eq!(serde_json::to_string(&Agent::Codex).unwrap(), "\"codex\"");
    }
}

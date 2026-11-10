//! Shared by sender and receiver so a new field can't be silently dropped.

use serde::{Deserialize, Serialize};

/// The events Tethys answers, each with the name Claude Code expects back in
/// `hookEventName`. Any other event is fire-and-forget.
const REPLYING_EVENTS: &[(&str, &str)] = &[
    ("post-tool", "PostToolUse"),
    ("post-tool-failure", "PostToolUseFailure"),
    ("user-submit", "UserPromptSubmit"),
];

pub fn reply_event_name(event: &str) -> Option<&'static str> {
    REPLYING_EVENTS
        .iter()
        .find(|(ours, _)| *ours == event)
        .map(|(_, claude)| *claude)
}

/// All optional: an agent's schema shift must never break either side.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HookMessage {
    /// Tethys's name for the event (`session-start`), not the agent's.
    pub event: String,
    /// Rotates on compaction and resume.
    pub session_id: Option<String>,
    /// Survives the id rotation, so it's the fallback correlation key.
    pub cwd: Option<String>,
    pub transcript_path: Option<String>,
    pub source: Option<String>,
    pub notification_type: Option<String>,
    pub last_assistant_message: Option<String>,
    pub tool_name: Option<String>,
    /// The only fields of `tool_input` Tethys reads; the rest never crosses
    /// the socket.
    pub tool_file_path: Option<String>,
    pub tool_command: Option<String>,
    /// The tail of a tool's stdout, read only for ummon's review URL.
    pub tool_stdout: Option<String>,
    /// `None` for sessions Tethys didn't spawn.
    pub spawn_token: Option<String>,
    /// Claude's cross-session inbox, from `CLAUDE_CODE_MESSAGING_*`.
    pub messaging_socket: Option<String>,
    pub messaging_token: Option<String>,
}

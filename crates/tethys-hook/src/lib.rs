//! Shared by sender and receiver so a new field can't be silently dropped.

use serde::{Deserialize, Serialize};

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
    pub hook_event_name: Option<String>,
    pub source: Option<String>,
    pub message: Option<String>,
    pub notification_type: Option<String>,
    pub stop_hook_active: Option<bool>,
    pub last_assistant_message: Option<String>,
    pub tool_name: Option<String>,
    /// The only fields of `tool_input` Tethys reads; the rest never crosses
    /// the socket.
    pub tool_file_path: Option<String>,
    pub tool_command: Option<String>,
    /// `None` for sessions Tethys didn't spawn.
    pub spawn_token: Option<String>,
}

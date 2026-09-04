//! The wire format shared by the `tethys-hook` companion binary and the
//! Tethys app that receives its frames.
//!
//! Both sides depend on this one type so a field can't be added to the sender
//! and silently dropped by the receiver — which is exactly what happened to
//! `cwd`, the one field that survives Claude rotating its session id.

use serde::{Deserialize, Serialize};

/// One hook event, flattened (no nesting) for simpler parsing.
///
/// Every field is optional: unknown hook events and schema shifts on Claude
/// Code's side must never break the sender or the receiver.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HookMessage {
    /// Tethys's own short name for the event (`session-start`, `stop`, …),
    /// derived from the hook's position in `settings.json`.
    pub event: String,
    /// Claude's session id. Rotates on compaction/resume.
    pub session_id: Option<String>,
    /// The session's working directory. Stable across the id rotation, so it
    /// is the correlation key to fall back on.
    pub cwd: Option<String>,
    pub transcript_path: Option<String>,
    /// Claude's own name for the event (`SessionStart`, `Stop`, …).
    pub hook_event_name: Option<String>,
    pub source: Option<String>,
    pub message: Option<String>,
    pub notification_type: Option<String>,
    pub stop_hook_active: Option<bool>,
    pub last_assistant_message: Option<String>,
    /// PreToolUse / PostToolUse only: the tool Claude ran (`Write`, `Bash`, …).
    pub tool_name: Option<String>,
    /// PreToolUse / PostToolUse only: `tool_input.file_path` (file tools) and
    /// `tool_input.command` (Bash), flattened out of the tool's own argument
    /// object. The only fields of any tool's input Tethys reads, so the rest
    /// of the object never crosses the socket.
    pub tool_file_path: Option<String>,
    pub tool_command: Option<String>,
    /// Tethys-injected: matches the UUID set as `TETHYS_SPAWN_TOKEN` on the
    /// PTY. `None` for sessions Tethys didn't spawn.
    pub spawn_token: Option<String>,
}

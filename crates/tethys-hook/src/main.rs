//! Claude Code hook companion binary.
//!
//! Installed by Tethys into `~/.claude/settings.json`. When Claude Code fires
//! a hook (SessionStart / Stop / Notification), this process:
//!
//! 1. Reads the hook payload (JSON) from stdin.
//! 2. Reads the `TETHYS_SPAWN_TOKEN` env var if present (set only on
//!    sessions Tethys itself spawned — lets the backend correlate Claude's
//!    `session_id` back to the session it's tracking).
//! 3. Opens `~/Library/Application Support/app.tethys.dev/hook.sock` and
//!    sends a length-prefixed JSON frame.
//! 4. Exits 0 no matter what. If Tethys isn't running, or the socket is
//!    missing, or the JSON is malformed — the user's Claude session must
//!    never be disrupted.

use std::env;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

use tethys_hook::HookMessage;

/// Fields we care about from Claude Code's hook payload. Every field is
/// optional so unknown hook events or schema shifts don't break us.
#[derive(Default, Deserialize)]
#[serde(default)]
struct HookInput {
    session_id: Option<String>,
    cwd: Option<String>,
    transcript_path: Option<String>,
    hook_event_name: Option<String>,
    source: Option<String>,
    message: Option<String>,
    notification_type: Option<String>,
    stop_hook_active: Option<bool>,
    last_assistant_message: Option<String>,
}

fn main() {
    // Never bubble errors out — we'd disrupt Claude for no gain.
    let _ = run();
}

fn run() -> io::Result<()> {
    let event = env::args().nth(1).unwrap_or_default();
    if event.is_empty() {
        return Ok(());
    }

    let mut stdin_buf = String::new();
    io::stdin().read_to_string(&mut stdin_buf)?;
    let input: HookInput = serde_json::from_str(&stdin_buf).unwrap_or_default();

    let msg = HookMessage {
        event,
        session_id: input.session_id,
        cwd: input.cwd,
        transcript_path: input.transcript_path,
        hook_event_name: input.hook_event_name,
        source: input.source,
        message: input.message,
        notification_type: input.notification_type,
        stop_hook_active: input.stop_hook_active,
        last_assistant_message: input.last_assistant_message,
        spawn_token: env::var("TETHYS_SPAWN_TOKEN").ok(),
    };

    let socket_path = socket_path()?;
    let mut stream = match UnixStream::connect(&socket_path) {
        Ok(s) => s,
        Err(_) => return Ok(()), // Tethys not running; silent no-op
    };
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;

    let payload = match serde_json::to_vec(&msg) {
        Ok(b) => b,
        Err(_) => return Ok(()),
    };
    let len = (payload.len() as u32).to_be_bytes();
    stream.write_all(&len)?;
    stream.write_all(&payload)?;
    Ok(())
}

fn socket_path() -> io::Result<PathBuf> {
    // macOS-only for MVP. Mirrors `Paths::hook_socket_path()` on the Rust
    // backend side.
    let home = env::var_os("HOME")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME not set"))?;
    Ok(PathBuf::from(home)
        .join("Library")
        .join("Application Support")
        .join("app.tethys.dev")
        .join("hook.sock"))
}

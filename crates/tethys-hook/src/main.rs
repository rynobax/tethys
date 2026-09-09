//! Agent hook companion binary — one binary for both harnesses.
//!
//! Claude Code reaches it through `~/.claude/settings.json`; codex through
//! `-c hooks.<Event>=…` on its own command line. Either way, when a hook
//! fires, this process:
//!
//! 1. Reads the hook payload (JSON) from stdin.
//! 2. Works out the spawn token, which correlates the agent's own session id
//!    back to the session Tethys is tracking. Claude gets it from the
//!    `TETHYS_SPAWN_TOKEN` env var, set on the tmux session. Codex gets it
//!    from `--spawn-token` on this command line, because Tethys writes that
//!    command line itself and an argument can't be filtered out by an
//!    environment policy the way a var can.
//! 3. Opens `~/Library/Application Support/app.tethys.dev/hook.sock` and
//!    sends a length-prefixed JSON frame.
//! 4. Exits 0 no matter what. If Tethys isn't running, or the socket is
//!    missing, or the JSON is malformed — the user's session must never be
//!    disrupted.
//!
//! The two payloads are near-identical: both send `session_id`, `cwd`,
//! `transcript_path`, `hook_event_name`, `last_assistant_message` and
//! `tool_name`, and both nest a tool's arguments under `tool_input`. Only the
//! event *names* and the set of events differ, and those are the caller's
//! problem, not this binary's.

use std::env;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

use tethys_hook::HookMessage;

/// Fields we care about from an agent's hook payload. Every field is optional
/// so unknown hook events or schema shifts don't break us — and so one struct
/// can read both harnesses, each of which simply omits what it doesn't send.
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
    tool_name: Option<String>,
    tool_input: Option<ToolInput>,
}

/// The two fields of a tool's argument object Tethys cares about.
#[derive(Default, Deserialize)]
#[serde(default)]
struct ToolInput {
    file_path: Option<String>,
    command: Option<String>,
}

fn main() {
    // Never bubble errors out — we'd disrupt the session for no gain.
    let _ = run();
}

fn run() -> io::Result<()> {
    let Some((event, spawn_token)) = parse_args(env::args().skip(1)) else {
        return Ok(());
    };

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
        tool_name: input.tool_name,
        tool_file_path: input.tool_input.as_ref().and_then(|t| t.file_path.clone()),
        tool_command: input.tool_input.and_then(|t| t.command),
        spawn_token: spawn_token.or_else(|| env::var("TETHYS_SPAWN_TOKEN").ok()),
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

/// `tethys-hook <event> [--spawn-token <token>]`.
///
/// `None` when there's no event to report, which is the only thing worth
/// refusing on: an unrecognised flag is ignored rather than fatal, since
/// exiting non-zero here would surface as a hook failure in the user's
/// session.
fn parse_args(args: impl Iterator<Item = String>) -> Option<(String, Option<String>)> {
    let mut event = None;
    let mut spawn_token = None;
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--spawn-token" => spawn_token = args.next(),
            _ if arg.starts_with("--spawn-token=") => {
                spawn_token = arg.split_once('=').map(|(_, v)| v.to_string());
            }
            _ if arg.starts_with('-') => {}
            _ if event.is_none() => event = Some(arg),
            _ => {}
        }
    }
    event.filter(|e| !e.is_empty()).map(|e| (e, spawn_token))
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

#[cfg(test)]
mod tests {
    use super::parse_args;

    fn parse(args: &[&str]) -> Option<(String, Option<String>)> {
        parse_args(args.iter().map(|s| s.to_string()))
    }

    /// Claude's form: the event alone, with the token arriving via the
    /// environment instead.
    #[test]
    fn an_event_on_its_own_is_enough() {
        assert_eq!(parse(&["stop"]), Some(("stop".into(), None)));
    }

    /// Codex's form. Both spellings, because Tethys renders the command into a
    /// TOML string and the separated form is the one that survives least
    /// ambiguously.
    #[test]
    fn the_spawn_token_is_read_either_way() {
        let expected = Some(("stop".into(), Some("tok-1".into())));
        assert_eq!(parse(&["stop", "--spawn-token", "tok-1"]), expected);
        assert_eq!(parse(&["stop", "--spawn-token=tok-1"]), expected);
        assert_eq!(parse(&["--spawn-token", "tok-1", "stop"]), expected);
    }

    /// Nothing to report is the only refusal. An unknown flag is skipped
    /// rather than fatal: exiting non-zero would surface as a hook failure in
    /// the user's session, which is a worse outcome than ignoring a flag.
    #[test]
    fn no_event_is_the_only_refusal() {
        assert_eq!(parse(&[]), None);
        assert_eq!(parse(&["--spawn-token", "tok-1"]), None);
        assert_eq!(
            parse(&["--future-flag", "stop"]),
            Some(("stop".into(), None))
        );
    }
}

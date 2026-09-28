//! Forwards an agent hook's payload to Tethys over `hook.sock`, for both
//! Claude and codex. Always exits 0: a hook must never disrupt the session.

use std::env;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

use tethys_hook::HookMessage;

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

#[derive(Default, Deserialize)]
#[serde(default)]
struct ToolInput {
    file_path: Option<String>,
    command: Option<String>,
}

fn main() {
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
        Err(_) => return Ok(()),
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

/// `tethys-hook <event> [--spawn-token <token>]`. Unknown flags are ignored.
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
    // Mirrors `Paths::hook_socket`.
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

    /// Claude's form; its token arrives via the environment.
    #[test]
    fn an_event_on_its_own_is_enough() {
        assert_eq!(parse(&["stop"]), Some(("stop".into(), None)));
    }

    /// Codex's form.
    #[test]
    fn the_spawn_token_is_read_either_way() {
        let expected = Some(("stop".into(), Some("tok-1".into())));
        assert_eq!(parse(&["stop", "--spawn-token", "tok-1"]), expected);
        assert_eq!(parse(&["stop", "--spawn-token=tok-1"]), expected);
        assert_eq!(parse(&["--spawn-token", "tok-1", "stop"]), expected);
    }

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

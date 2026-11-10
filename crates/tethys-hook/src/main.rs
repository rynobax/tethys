//! Always exits 0: a hook must never disrupt the session.

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
    source: Option<String>,
    notification_type: Option<String>,
    last_assistant_message: Option<String>,
    tool_name: Option<String>,
    tool_input: Option<ToolInput>,
    tool_response: Option<serde_json::Value>,
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
        source: input.source,
        notification_type: input.notification_type,
        last_assistant_message: input.last_assistant_message,
        tool_name: input.tool_name,
        tool_file_path: input.tool_input.as_ref().and_then(|t| t.file_path.clone()),
        tool_command: input.tool_input.and_then(|t| t.command),
        tool_stdout: input.tool_response.as_ref().and_then(stdout_tail),
        spawn_token: spawn_token.or_else(|| env::var("TETHYS_SPAWN_TOKEN").ok()),
        messaging_socket: env::var("CLAUDE_CODE_MESSAGING_SOCKET").ok(),
        messaging_token: env::var("CLAUDE_CODE_MESSAGING_TOKEN").ok(),
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

    let Some(claude_event) = tethys_hook::reply_event_name(&msg.event) else {
        return Ok(());
    };
    // The agent waits on this hook, so a wedged Tethys mustn't stall it.
    stream.set_read_timeout(Some(Duration::from_millis(500)))?;
    let context = read_reply(&mut stream)?;
    if !context.is_empty() {
        let output = serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": claude_event,
                "additionalContext": context,
            }
        });
        println!("{output}");
    }
    Ok(())
}

/// Claude's Bash reply is `{stdout, …}`; codex's is a bare string. Only the
/// tail crosses the socket, since what Tethys looks for is printed last.
fn stdout_tail(response: &serde_json::Value) -> Option<String> {
    const MAX: usize = 8 * 1024;
    let out = response
        .get("stdout")
        .and_then(|v| v.as_str())
        .or_else(|| response.as_str())?;
    let mut start = out.len().saturating_sub(MAX);
    while !out.is_char_boundary(start) {
        start += 1;
    }
    Some(out[start..].to_string())
}

/// Length-prefixed like the request; empty means nothing to say.
fn read_reply(stream: &mut UnixStream) -> io::Result<String> {
    let mut len = [0u8; 4];
    stream.read_exact(&mut len)?;
    let mut buf = vec![0u8; u32::from_be_bytes(len) as usize];
    stream.read_exact(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
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

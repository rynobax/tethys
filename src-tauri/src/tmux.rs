use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::AppResult;
use crate::shell;

/// A server of our own, so the user's `~/.tmux.conf` and sessions stay out of it.
pub const SOCKET_LABEL: &str = "tethys";

pub struct TmuxBin(pub PathBuf);

pub fn resolve() -> AppResult<PathBuf> {
    shell::which("tmux", Some("brew install tmux"))
}

/// Placeholder until xterm.js mounts and resizes the pane.
const INITIAL_COLS: &str = "200";
const INITIAL_ROWS: &str = "50";

/// [`server_init_args`] must be prepended: on cold start `new-session` boots
/// the server, and options set afterwards would miss it.
pub fn new_session_args(
    session_id: &str,
    env: &[(&str, String)],
    command: &[String],
) -> Vec<String> {
    let mut args: Vec<String> = vec!["-L".into(), SOCKET_LABEL.into()];
    args.extend(server_init_args());
    args.extend([
        "new-session".into(),
        "-A".into(),
        "-D".into(),
        "-s".into(),
        session_id.to_string(),
    ]);
    for (key, value) in env {
        args.push("-e".into());
        args.push(format!("{key}={value}"));
    }
    args.extend([
        "-x".into(),
        INITIAL_COLS.into(),
        "-y".into(),
        INITIAL_ROWS.into(),
        "--".into(),
    ]);
    args.extend(command.iter().cloned());
    args
}

pub fn attach_session_args(session_id: &str) -> Vec<String> {
    let mut args: Vec<String> = vec!["-L".into(), SOCKET_LABEL.into()];
    args.extend(server_init_args());
    args.extend([
        "attach-session".into(),
        "-d".into(),
        "-t".into(),
        session_id.to_string(),
    ]);
    args
}

pub fn has_session(tmux_bin: &Path, session_id: &str) -> bool {
    Command::new(tmux_bin)
        .args(["-L", SOCKET_LABEL, "has-session", "-t", session_id])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Server-global options as a `;`-chained command prefix.
pub fn server_init_args() -> Vec<String> {
    let commands: &[&[&str]] = &[
        &["set-option", "-g", "window-size", "latest"],
        &["set-option", "-g", "status", "off"],
        // Claude's fullscreen renderer requests mouse tracking to own
        // scrolling; tmux only forwards it with `mouse on`.
        &["set-option", "-g", "mouse", "on"],
        // Bounds `capture-pane -S -`, i.e. what a reattach can replay.
        &["set-option", "-g", "history-limit", "50000"],
        // Keep the client out of xterm.js's alternate buffer, which has no
        // scrollback for panes that don't handle the mouse themselves.
        // `-g` with the default `linux*:AX@` spelled out, since `-ga` on
        // every spawn grew the array without bound on a long-lived server.
        &[
            "set-option",
            "-g",
            "terminal-overrides",
            "linux*:AX@,*:smcup@:rmcup@",
        ],
    ];
    commands
        .iter()
        .flat_map(|cmd| cmd.iter().map(|s| s.to_string()).chain(std::iter::once(";".into())))
        .collect()
}

/// Best-effort for a server surviving from a prior run; every spawn also
/// prepends the options.
pub fn ensure_server_init(tmux_bin: &Path) {
    let _ = Command::new(tmux_bin)
        .args(["-L", SOCKET_LABEL])
        .args(server_init_args())
        .status();
}

pub fn capture_pane(tmux_bin: &Path, session_id: &str) -> Option<Vec<u8>> {
    let output = Command::new(tmux_bin)
        .args([
            "-L",
            SOCKET_LABEL,
            "capture-pane",
            "-p",
            "-e",
            "-S", "-",
            "-t", session_id,
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let mut out = Vec::with_capacity(output.stdout.len() + 8);
    for &b in &output.stdout {
        if b == b'\n' {
            out.push(b'\r');
        }
        out.push(b);
    }
    // So lingering attributes don't bleed into tmux's first paint.
    out.extend_from_slice(b"\x1b[0m");
    Some(out)
}

pub fn list_sessions(tmux_bin: &Path) -> Vec<String> {
    try_list_sessions(tmux_bin).unwrap_or_default()
}

/// `None` when tmux can't answer, which callers acting on *absence* from the
/// list must not mistake for "no sessions".
pub fn try_list_sessions(tmux_bin: &Path) -> Option<Vec<String>> {
    let output = Command::new(tmux_bin)
        .args([
            "-L",
            SOCKET_LABEL,
            "list-sessions",
            "-F",
            "#{session_name}",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect(),
    )
}

/// The session's own environment — what `new-session -e` set — as `KEY=value`
/// lines. `None` when tmux can't answer.
pub fn session_environment(tmux_bin: &Path, session_id: &str) -> Option<Vec<String>> {
    let output = Command::new(tmux_bin)
        .args(["-L", SOCKET_LABEL, "show-environment", "-t", session_id])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::to_string)
            .collect(),
    )
}

pub fn kill_session(tmux_bin: &Path, session_id: &str) {
    let _ = Command::new(tmux_bin)
        .args(["-L", SOCKET_LABEL, "kill-session", "-t", session_id])
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_options_are_prepended_before_the_subcommand() {
        let args = new_session_args("sess-1", &[], &["/bin/zsh".into()]);
        let new_session = args.iter().position(|a| a == "new-session").unwrap();
        let mouse = args.iter().position(|a| a == "mouse").unwrap();
        assert!(mouse < new_session, "{args:?}");

        let args = attach_session_args("sess-1");
        let attach = args.iter().position(|a| a == "attach-session").unwrap();
        let mouse = args.iter().position(|a| a == "mouse").unwrap();
        assert!(mouse < attach, "{args:?}");
    }

    #[test]
    fn both_argv_builders_target_the_tethys_socket() {
        for args in [
            new_session_args("s", &[], &["cmd".into()]),
            attach_session_args("s"),
        ] {
            assert_eq!(&args[0], "-L");
            assert_eq!(&args[1], SOCKET_LABEL);
        }
    }

    #[test]
    fn new_session_carries_env_and_command() {
        let args = new_session_args(
            "sess-1",
            &[("TETHYS_SPAWN_TOKEN", "tok-123".to_string())],
            &["/bin/zsh".into(), "-lc".into(), "yarn dev".into()],
        );

        let e = args.iter().position(|a| a == "-e").unwrap();
        assert_eq!(args[e + 1], "TETHYS_SPAWN_TOKEN=tok-123");

        let sep = args.iter().position(|a| a == "--").unwrap();
        assert_eq!(&args[sep + 1..], ["/bin/zsh", "-lc", "yarn dev"]);
    }
}

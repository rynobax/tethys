use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::AppResult;
use crate::shell;

/// Socket label for the tmux server Tethys uses. Kept distinct from the
/// user's personal tmux so their `~/.tmux.conf` isn't loaded, their
/// keybindings don't collide, and we can query/kill our own server without
/// touching their setup.
pub const SOCKET_LABEL: &str = "tethys";

/// Newtype managed in Tauri state, like `ClaudeBin`.
pub struct TmuxBin(pub PathBuf);

/// Resolve the absolute path to `tmux` via a login shell — desktop apps on
/// macOS don't inherit Homebrew's bin dir in PATH.
pub fn resolve() -> AppResult<PathBuf> {
    shell::which("tmux", Some("brew install tmux"))
}

/// Default pane size for a freshly-created tmux session. xterm.js resizes it
/// to the real geometry as soon as the pane mounts.
const INITIAL_COLS: &str = "200";
const INITIAL_ROWS: &str = "50";

/// argv for creating a tmux session named `session_id` running `command`.
///
/// `env` becomes per-session `-e` variables — how `TETHYS_SPAWN_TOKEN` reaches
/// Claude so its SessionStart hook can be correlated back.
///
/// Built here rather than at each call site because the sessions and scripts
/// supervisors used to construct this argv separately, character for
/// character, including the load-bearing detail that [`server_init_args`] must
/// be *prepended*: on cold start `new-session` is what boots the server, so
/// options set afterwards would miss it and the pane would silently come up
/// without `mouse on`.
pub fn new_session_args(
    session_id: &str,
    env: &[(&str, String)],
    command: &[String],
) -> Vec<String> {
    let mut args: Vec<String> = vec!["-L".into(), SOCKET_LABEL.into()];
    args.extend(server_init_args());
    args.extend([
        "new-session".into(),
        "-A".into(), // attach if a session with this name somehow exists
        "-D".into(), // ...and detach any other clients on that session
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

/// argv for attaching a fresh client to an existing tmux session, detaching
/// any other client first.
pub fn attach_session_args(session_id: &str) -> Vec<String> {
    let mut args: Vec<String> = vec!["-L".into(), SOCKET_LABEL.into()];
    args.extend(server_init_args());
    args.extend([
        "attach-session".into(),
        "-d".into(), // detach any other clients
        "-t".into(),
        session_id.to_string(),
    ]);
    args
}

/// `true` if the tmux session named `session_id` exists on the Tethys
/// server. Uses `tmux -L tethys has-session -t <id>` — exit 0 = exists.
pub fn has_session(tmux_bin: &Path, session_id: &str) -> bool {
    Command::new(tmux_bin)
        .args(["-L", SOCKET_LABEL, "has-session", "-t", session_id])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Server-global options, as a tmux multi-command prefix. Prepending
/// these (separated by `;`) to `new-session`/`attach-session` ensures the
/// options apply on cold-start too — at boot the server may not exist,
/// so `set-option` before `new-session` runs against the just-started
/// server.
///
/// - `window-size latest` sizes windows to the most-recently-attached
///   client rather than the min across clients. With a single Tethys
///   client this matches the terminal exactly.
/// - `status off` hides tmux's status bar — Tethys' UI already shows
///   session info, so the bar is just a wasted row of screen.
/// - `mouse on` — required by Claude Code's fullscreen renderer. It draws
///   on the pane's alternate screen and requests SGR mouse tracking, so it
///   owns scrolling. With `mouse off` tmux never enables mouse reporting on
///   xterm.js, the wheel is handled locally by xterm.js against its own
///   (now stale) buffer, and Claude's viewport never moves. tmux only
///   grabs the wheel for copy-mode when the pane app hasn't asked for
///   mouse tracking; when it has, tmux forwards the events through.
pub fn server_init_args() -> Vec<String> {
    // Each inner slice is one tmux command; `;` in between is tmux's
    // command-chain separator. Tmux is purely a process keeper here —
    // xterm.js is a display surface, and the pane app decides who handles
    // the mouse.
    let commands: &[&[&str]] = &[
        &["set-option", "-g", "window-size", "latest"],
        &["set-option", "-g", "status", "off"],
        // Explicitly set — a previous run may have flipped it, and the
        // tmux server survives across Tethys restarts.
        &["set-option", "-g", "mouse", "on"],
        // `capture-pane -S -` is bounded by history-limit; bump it so
        // cross-restart reattach has plenty of history to replay into
        // xterm.js.
        &["set-option", "-g", "history-limit", "50000"],
        // Strip alt-screen (smcup/rmcup) from every terminal's terminfo.
        // Without this, tmux-the-client enters alt-screen on xterm.js the
        // moment it attaches, flipping xterm.js into its alternate buffer
        // for the whole session — which has no scrollback, so panes that
        // don't capture the mouse themselves (a plain shell, Claude's
        // classic renderer) lose wheel scrolling entirely. Keeping
        // everything in the main buffer leaves xterm's own scrollback as
        // the fallback for those.
        //
        // `-g` (not `-ga`) with the default `linux*:AX@` spelled out:
        // `server_init_args()` runs on every spawn and the tmux server
        // outlives Tethys, so appending grew this array without bound.
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

/// Boot-time best-effort: apply server options if the server is already
/// up from a prior run. Safe to skip failures — `server_init_args()` is
/// also prepended to every spawn, which covers cold-start.
pub fn ensure_server_init(tmux_bin: &Path) {
    let _ = Command::new(tmux_bin)
        .args(["-L", SOCKET_LABEL])
        .args(server_init_args())
        .status();
}

/// Dump a session's pane scrollback + visible buffer, with SGR
/// preserved. Returns `None` if the session doesn't exist or the command
/// fails. Output has `\n` converted to `\r\n` for xterm.js, and an SGR
/// reset appended so lingering attributes don't bleed into the client's
/// redraw.
pub fn capture_pane(tmux_bin: &Path, session_id: &str) -> Option<Vec<u8>> {
    let output = Command::new(tmux_bin)
        .args([
            "-L",
            SOCKET_LABEL,
            "capture-pane",
            "-p",       // print to stdout (don't save to buffer)
            "-e",       // preserve escape sequences (SGR)
            "-S", "-",  // start at top of history
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
    // Reset SGR so tmux's first real paint starts from a known state.
    out.extend_from_slice(b"\x1b[0m");
    Some(out)
}

/// Return the names of all sessions on the Tethys tmux server. Empty vec
/// if the server isn't running or has no sessions.
pub fn list_sessions(tmux_bin: &Path) -> Vec<String> {
    let output = Command::new(tmux_bin)
        .args([
            "-L",
            SOCKET_LABEL,
            "list-sessions",
            "-F",
            "#{session_name}",
        ])
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Best-effort kill of a tmux session by name. Silent on failure.
pub fn kill_session(tmux_bin: &Path, session_id: &str) {
    let _ = Command::new(tmux_bin)
        .args(["-L", SOCKET_LABEL, "kill-session", "-t", session_id])
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rule that made this worth extracting: server options must come
    /// before `new-session`, because on cold start `new-session` is what boots
    /// the server. Get the order wrong and the pane comes up without
    /// `mouse on`, which breaks scrolling in Claude's fullscreen renderer —
    /// silently, and only on the first launch after a reboot.
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

        // Everything after `--` is the command, in order.
        let sep = args.iter().position(|a| a == "--").unwrap();
        assert_eq!(&args[sep + 1..], ["/bin/zsh", "-lc", "yarn dev"]);
    }

    #[test]
    fn a_session_with_no_env_has_no_e_flags() {
        let args = new_session_args("sess-1", &[], &["claude".into()]);
        assert!(!args.iter().any(|a| a == "-e"), "{args:?}");
        assert_eq!(args.last().unwrap(), "claude");
    }

    #[test]
    fn attach_detaches_other_clients() {
        let args = attach_session_args("sess-1");
        let t = args.iter().position(|a| a == "-t").unwrap();
        assert_eq!(args[t + 1], "sess-1");
        assert!(args.contains(&"-d".to_string()));
    }
}

//! The argv Tethys spawns an agent CLI with.
//!
//! One function per agent, because the two command lines have almost nothing
//! in common beyond the binary and the trailing prompt. Claude Code takes a
//! `--resume` flag and an inline `--mcp-config`, and reads its hooks from
//! `~/.claude/settings.json`. Codex resumes through a subcommand and takes
//! MCP servers, hooks and everything else as `-c` config overrides. Trying to
//! express both as one parameterised builder produces a shape that describes
//! neither.

use std::path::{Path, PathBuf};

use crate::agent::Agent;
use crate::error::AppResult;
use crate::mcp::McpLaunch;

/// Codex hook events Tethys listens for, paired with the `tethys-hook`
/// subcommand each maps to.
///
/// A subset of the twelve codex offers — the ones that answer "is this session
/// working, idle, or waiting on me", plus the two that carry artifacts. Codex
/// has no `Notification`; `PermissionRequest` covers the same ground and is
/// already a signal Tethys understands from Claude. `Interrupt` is the one
/// event with no Claude counterpart worth registering: it's the user pressing
/// escape, which ends a turn as surely as `Stop` does.
const CODEX_HOOK_EVENTS: &[(&str, &str)] = &[
    ("SessionStart", "session-start"),
    ("UserPromptSubmit", "user-submit"),
    ("PreToolUse", "pre-tool"),
    ("PostToolUse", "post-tool"),
    ("Stop", "stop"),
    ("PermissionRequest", "permission-request"),
    ("Interrupt", "interrupt"),
];

/// What a spawn needs to know to render a command line.
pub struct Spawn<'a> {
    pub agent: Agent,
    /// The already-resolved binary path.
    pub agent_bin: &'a Path,
    pub workspace_id: &'a str,
    /// Tethys's id for the session — also the tmux session name. Baked into
    /// the MCP config so a tool call can name its own session.
    pub session_id: &'a str,
    /// Correlates the agent's session-start hook back to this spawn. Reaches
    /// Claude as an environment variable on the tmux session; reaches codex as
    /// an argument on the hook command lines below.
    pub spawn_token: &'a str,
    /// The `tethys-hook` companion binary, for the agent that takes its hooks
    /// on the command line. `None` when it isn't installed beside the app — a
    /// session then runs without turn tracking, which is degraded but not
    /// broken.
    pub hook_bin: Option<&'a Path>,
    /// Directories the agent must be able to write that aren't under its cwd.
    /// Each repo's real git dir lives in Tethys's data dir, so a sandboxed
    /// session couldn't commit without them.
    pub extra_writable: &'a [PathBuf],
    /// The *agent's* id for a conversation to resume, when there is one on
    /// disk to resume.
    pub resume_session_id: Option<&'a str>,
    pub mcp: Option<&'a McpLaunch>,
    /// The Brief, for the session a handoff creates.
    pub brief: Option<&'a str>,
}

pub fn build(spawn: Spawn<'_>) -> AppResult<Vec<String>> {
    match spawn.agent {
        Agent::Claude => Ok(claude(spawn)),
        Agent::Codex => codex(spawn),
    }
}

/// `claude [--resume <id>] [--mcp-config=… --allowed-tools=…] [prompt]`
fn claude(spawn: Spawn<'_>) -> Vec<String> {
    let mut command = vec![spawn.agent_bin.to_string_lossy().into_owned()];
    if let Some(sid) = spawn.resume_session_id {
        command.push("--resume".into());
        command.push(sid.to_string());
    }
    // Rendered here rather than by the caller because the config bakes in the
    // session id, and the caller is where that id is minted.
    if let Some(mcp) = spawn.mcp {
        command.extend(mcp.claude_args(spawn.workspace_id, spawn.session_id));
    }
    // The Brief goes last: `claude [options] [prompt]`. tmux passes argv
    // through verbatim, so a multi-line brief with quotes in it survives.
    if let Some(brief) = spawn.brief {
        command.push(brief.to_string());
    }
    command
}

/// `codex [flags] [-c override]… [resume <id>] [prompt]`
///
/// Everything Tethys needs is expressed per-invocation. Codex layers config
/// from several files, and `-c` is the highest-precedence layer, so nothing
/// has to be written to `~/.codex/config.toml` — which keeps the user's own
/// codex config theirs, keeps a session's whole configuration visible in one
/// command line, and means running `codex` by hand behaves like plain codex.
fn codex(spawn: Spawn<'_>) -> AppResult<Vec<String>> {
    let mut command = vec![spawn.agent_bin.to_string_lossy().into_owned()];

    // A hook defined on the command line starts untrusted and silently does
    // not run. The alternative to this flag is writing a `[hooks.state]`
    // stanza per spawn into the user's global config, hashed over a command
    // string that carries a fresh spawn token every time — an unbounded pile
    // of dead entries. The flag's scope is the hooks on this very command
    // line, which Tethys wrote.
    command.push("--dangerously-bypass-hook-trust".into());

    // The faithful equivalent of plain `claude`: free to edit its own
    // workspace, asks before anything else. Spelled out because a worktree is
    // an untrusted project to codex, and an untrusted project otherwise
    // defaults to read-only — a session that can't edit anything.
    command.push("--sandbox".into());
    command.push("workspace-write".into());
    command.push("--ask-for-approval".into());
    command.push("on-request".into());

    for dir in spawn.extra_writable {
        command.push("--add-dir".into());
        command.push(dir.to_string_lossy().into_owned());
    }

    // Tethys generates one workspace doc, and it is called CLAUDE.md. Codex
    // reads AGENTS.md, so point it at the file that's actually there rather
    // than generating a second copy under another name for the two to drift
    // apart. The session runs at the workspace root, so the doc is in its cwd
    // and gets read directly — codex's own walk stops at the git root, which
    // a workspace root is not.
    command.push("-c".into());
    command.push("project_doc_fallback_filenames=[\"CLAUDE.md\"]".into());

    if let Some(hook_bin) = spawn.hook_bin {
        command.extend(codex_hook_args(hook_bin, spawn.spawn_token));
    }
    if let Some(mcp) = spawn.mcp {
        command.extend(mcp.codex_args(spawn.workspace_id, spawn.session_id));
    }

    // `resume` is a subcommand, not a flag, so every global option above has
    // to precede it — and the prompt, which codex takes positionally, has to
    // follow it.
    if let Some(sid) = spawn.resume_session_id {
        command.push("resume".into());
        command.push(sid.to_string());
    }
    if let Some(brief) = spawn.brief {
        command.push(brief.to_string());
    }
    Ok(command)
}

/// One `-c hooks.<Event>=…` per event, each running `tethys-hook <subcommand>
/// --spawn-token <token>`.
///
/// The token is an argument rather than an environment variable because Tethys
/// writes this command line itself, and codex's `shell_environment_policy` can
/// filter what a subprocess inherits. An argument can't be filtered away.
///
/// A handler's `command` is a *shell string*, not an argv array — codex
/// rejects the array form outright ("invalid type: sequence, expected a
/// string"), and rejects it while loading config, so the whole session dies
/// before the TUI starts. Each word is therefore single-quoted on its way in:
/// the hook binary's path runs through the app bundle's name and is the one
/// part a stray space would silently break.
fn codex_hook_args(hook_bin: &Path, spawn_token: &str) -> Vec<String> {
    let bin = shell_word(&hook_bin.to_string_lossy());
    let token = shell_word(spawn_token);
    CODEX_HOOK_EVENTS
        .iter()
        .flat_map(|(event, subcommand)| {
            let command = format!("{bin} {} --spawn-token {token}", shell_word(subcommand));
            [
                "-c".to_string(),
                format!(
                    "hooks.{event}=[{{hooks=[{{type=\"command\",command={}}}]}}]",
                    toml_string(&command)
                ),
            ]
        })
        .collect()
}

/// Single-quote a word for the shell codex runs a hook `command` through.
///
/// POSIX single quotes take everything literally and have exactly one escape:
/// end the quoting, emit an escaped quote, start again.
fn shell_word(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// Render a TOML basic string, for the `-c` overrides codex configures
/// everything through.
///
/// Most values Tethys puts in one are its own — a uuid, a repo key, a path
/// under the app bundle — but a workspace root carries a user-chosen branch
/// name, and an unescaped quote reaching a `-c` value is a parse error that
/// takes the whole spawn down rather than degrading anything.
pub fn toml_string(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn spawn<'a>(agent: Agent, resume: Option<&'a str>, brief: Option<&'a str>) -> Spawn<'a> {
        Spawn {
            agent,
            agent_bin: Path::new("/usr/local/bin/claude"),
            workspace_id: "ws-1",
            session_id: "sess-1",
            spawn_token: "tok-1",
            hook_bin: None,
            extra_writable: &[],
            resume_session_id: resume,
            mcp: None,
            brief,
        }
    }

    #[test]
    fn a_fresh_claude_is_just_the_binary() {
        let cmd = build(spawn(Agent::Claude, None, None)).unwrap();
        assert_eq!(cmd, vec![PathBuf::from("/usr/local/bin/claude").to_string_lossy()]);
    }

    /// The Brief is a positional argument and has to stay last, after every
    /// flag — `claude [options] [prompt]`.
    #[test]
    fn the_brief_goes_last() {
        let cmd = build(spawn(Agent::Claude, Some("csid-9"), Some("do the thing"))).unwrap();
        assert_eq!(&cmd[1..3], &["--resume".to_string(), "csid-9".to_string()]);
        assert_eq!(cmd.last().unwrap(), "do the thing");
    }

    /// Codex resumes through a *subcommand*, not a flag. Every global option
    /// has to come before it and the prompt after it, so a command line that
    /// puts `resume` anywhere but immediately before the Brief is broken —
    /// codex would reject the flags as unexpected arguments to the subcommand.
    #[test]
    fn codex_resume_is_a_subcommand_between_the_flags_and_the_brief() {
        let cmd = build(spawn(Agent::Codex, Some("uuid-7"), Some("do the thing"))).unwrap();
        let resume = cmd.iter().position(|a| a == "resume").expect("resume");

        assert_eq!(cmd[resume + 1], "uuid-7");
        assert_eq!(cmd.last().unwrap(), "do the thing");
        assert_eq!(cmd.len(), resume + 3, "nothing may follow the Brief");
    }

    /// A fresh codex session has no `resume` at all — passing the subcommand
    /// with no id would open codex's interactive session picker.
    #[test]
    fn a_fresh_codex_never_says_resume() {
        let cmd = build(spawn(Agent::Codex, None, None)).unwrap();
        assert!(!cmd.iter().any(|a| a == "resume"));
    }

    /// Without this, a hook defined on the command line is untrusted and
    /// silently never runs — which would cost every turn signal, the session
    /// id correlation, and artifacts, with no error anywhere.
    #[test]
    fn codex_hooks_are_bypassed_into_trust() {
        let cmd = build(spawn(Agent::Codex, None, None)).unwrap();
        assert!(cmd.iter().any(|a| a == "--dangerously-bypass-hook-trust"));
    }

    /// A worktree is an untrusted project to codex, and untrusted defaults to
    /// read-only. Spelling the sandbox out is what makes the session able to
    /// edit its own workspace at all.
    #[test]
    fn codex_gets_an_explicit_sandbox_and_approval_mode() {
        let cmd = build(spawn(Agent::Codex, None, None)).unwrap();
        let pair = |flag: &str| {
            cmd.iter()
                .position(|a| a == flag)
                .map(|i| cmd[i + 1].clone())
        };
        assert_eq!(pair("--sandbox").as_deref(), Some("workspace-write"));
        assert_eq!(pair("--ask-for-approval").as_deref(), Some("on-request"));
    }

    /// Every event Tethys tracks has to reach `tethys-hook` carrying the spawn
    /// token, or the session-start hook can't be correlated back to the spawn
    /// that caused it.
    #[test]
    fn codex_hooks_carry_the_spawn_token() {
        let mut s = spawn(Agent::Codex, None, None);
        let hook_bin = PathBuf::from("/Applications/Tethys.app/tethys-hook");
        s.hook_bin = Some(&hook_bin);
        let cmd = build(s).unwrap();

        for (event, subcommand) in CODEX_HOOK_EVENTS {
            let rendered = cmd
                .iter()
                .find(|a| a.starts_with(&format!("hooks.{event}=")))
                .unwrap_or_else(|| panic!("no override for {event}"));
            assert!(rendered.contains(&format!("'{subcommand}'")), "{rendered}");
            assert!(rendered.contains("--spawn-token 'tok-1'"), "{rendered}");
            assert!(rendered.contains("'/Applications/Tethys.app/tethys-hook'"));
        }
    }

    /// No hook binary beside the app means no hooks at all rather than a
    /// broken override: turn tracking is degraded, the session still runs.
    #[test]
    fn no_hook_binary_means_no_hook_overrides() {
        let cmd = build(spawn(Agent::Codex, None, None)).unwrap();
        assert!(!cmd.iter().any(|a| a.starts_with("hooks.")));
    }

    /// A worktree's real git dir lives outside the workspace root, so without
    /// these the sandbox permits reading the checkout but not one git command.
    #[test]
    fn codex_is_granted_each_repos_git_dir() {
        let mut s = spawn(Agent::Codex, None, None);
        let dirs = vec![PathBuf::from("/data/repos/frontend.git")];
        s.extra_writable = &dirs;
        let cmd = build(s).unwrap();
        let i = cmd.iter().position(|a| a == "--add-dir").expect("--add-dir");
        assert_eq!(cmd[i + 1], "/data/repos/frontend.git");
    }

    /// Tethys generates one workspace doc and it is called CLAUDE.md. Codex
    /// has to be told to read it, or a codex workspace silently runs with no
    /// instructions about the worktree layout it's sitting in.
    #[test]
    fn codex_is_pointed_at_the_generated_doc() {
        let cmd = build(spawn(Agent::Codex, None, None)).unwrap();
        assert!(cmd
            .iter()
            .any(|a| a == r#"project_doc_fallback_filenames=["CLAUDE.md"]"#));
    }

    /// A handler's `command` is a shell string, and codex refuses an argv
    /// array *while loading config* — so getting this wrong doesn't degrade
    /// hooks, it kills the session before the TUI starts. That failure had no
    /// visible error in Tethys: the pane just exited 0.
    #[test]
    fn a_codex_hook_command_is_a_shell_string_not_an_array() {
        let mut s = spawn(Agent::Codex, None, None);
        let hook_bin = PathBuf::from("/Applications/Tethys.app/tethys-hook");
        s.hook_bin = Some(&hook_bin);
        let cmd = build(s).unwrap();
        let rendered = cmd
            .iter()
            .find(|a| a.starts_with("hooks.Stop="))
            .expect("Stop override");
        assert!(rendered.contains(r#"command=""#), "{rendered}");
        assert!(!rendered.contains("command=["), "{rendered}");
    }

    /// The shell runs the hook command, so a path with a space in it — an app
    /// bundle is one rename away from having one — must survive as one word.
    #[test]
    fn a_hook_path_with_a_space_stays_one_word() {
        let mut s = spawn(Agent::Codex, None, None);
        let hook_bin = PathBuf::from("/Applications/My Tethys.app/tethys-hook");
        s.hook_bin = Some(&hook_bin);
        let cmd = build(s).unwrap();
        let rendered = cmd
            .iter()
            .find(|a| a.starts_with("hooks.Stop="))
            .expect("Stop override");
        assert!(
            rendered.contains(r"'/Applications/My Tethys.app/tethys-hook'"),
            "{rendered}"
        );
    }

    #[test]
    fn shell_words_survive_a_quote() {
        assert_eq!(shell_word("plain"), "'plain'");
        assert_eq!(shell_word("it's"), r"'it'\''s'");
    }

    /// Paths run through the app bundle's name and a workspace id is a uuid,
    /// but a quote reaching an unquoted TOML value is a parse error that would
    /// take the whole spawn down.
    #[test]
    fn toml_values_are_quoted_and_escaped() {
        assert_eq!(toml_string("plain"), r#""plain""#);
        assert_eq!(toml_string(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(toml_string(r"back\slash"), r#""back\\slash""#);
    }
}

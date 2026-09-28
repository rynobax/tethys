use std::path::{Path, PathBuf};

use crate::agent::Agent;
use crate::error::AppResult;
use crate::mcp::McpLaunch;

/// Codex has no `Notification`; `PermissionRequest` covers the same ground.
/// `Interrupt` ends a turn like `Stop`.
const CODEX_HOOK_EVENTS: &[(&str, &str)] = &[
    ("SessionStart", "session-start"),
    ("UserPromptSubmit", "user-submit"),
    ("PreToolUse", "pre-tool"),
    ("PostToolUse", "post-tool"),
    ("Stop", "stop"),
    ("PermissionRequest", "permission-request"),
    ("Interrupt", "interrupt"),
];

pub struct Spawn<'a> {
    pub agent: Agent,
    pub agent_bin: &'a Path,
    pub workspace_id: &'a str,
    /// Tethys's id, which is also the tmux session name.
    pub session_id: &'a str,
    pub spawn_token: &'a str,
    /// Only codex takes its hooks on the command line.
    pub hook_bin: Option<&'a Path>,
    /// Each repo's git dir sits outside the cwd, under Tethys's data dir.
    pub extra_writable: &'a [PathBuf],
    /// The agent's own id, not Tethys's.
    pub resume_session_id: Option<&'a str>,
    pub mcp: Option<&'a McpLaunch>,
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
    if let Some(mcp) = spawn.mcp {
        command.extend(mcp.claude_args(spawn.workspace_id, spawn.session_id));
    }
    if let Some(brief) = spawn.brief {
        command.push(brief.to_string());
    }
    command
}

/// `codex [flags] [-c override]… [resume <id>] [prompt]`
///
/// `-c` outranks every config file, so the user's `~/.codex/config.toml` is
/// never written.
fn codex(spawn: Spawn<'_>) -> AppResult<Vec<String>> {
    let mut command = vec![spawn.agent_bin.to_string_lossy().into_owned()];

    // A command-line hook starts untrusted and silently doesn't run.
    command.push("--dangerously-bypass-hook-trust".into());

    // An untrusted project, which a worktree is, otherwise defaults to read-only.
    command.push("--sandbox".into());
    command.push("workspace-write".into());
    command.push("--ask-for-approval".into());
    command.push("on-request".into());

    for dir in spawn.extra_writable {
        command.push("--add-dir".into());
        command.push(dir.to_string_lossy().into_owned());
    }

    // The generated workspace doc is called CLAUDE.md.
    command.push("-c".into());
    command.push("project_doc_fallback_filenames=[\"CLAUDE.md\"]".into());

    if let Some(hook_bin) = spawn.hook_bin {
        command.extend(codex_hook_args(hook_bin, spawn.spawn_token));
    }
    if let Some(mcp) = spawn.mcp {
        command.extend(mcp.codex_args(spawn.workspace_id, spawn.session_id));
    }

    // A subcommand: global options must precede it and the prompt follow it.
    if let Some(sid) = spawn.resume_session_id {
        command.push("resume".into());
        command.push(sid.to_string());
    }
    if let Some(brief) = spawn.brief {
        command.push(brief.to_string());
    }
    Ok(command)
}

/// The token is an argument, not an env var, because `shell_environment_policy`
/// can filter env. `command` must be a shell string: codex rejects an argv
/// array while loading config, killing the session before its TUI starts.
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

fn shell_word(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

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

    #[test]
    fn the_brief_goes_last() {
        let cmd = build(spawn(Agent::Claude, Some("csid-9"), Some("do the thing"))).unwrap();
        assert_eq!(&cmd[1..3], &["--resume".to_string(), "csid-9".to_string()]);
        assert_eq!(cmd.last().unwrap(), "do the thing");
    }

    #[test]
    fn codex_resume_is_a_subcommand_between_the_flags_and_the_brief() {
        let cmd = build(spawn(Agent::Codex, Some("uuid-7"), Some("do the thing"))).unwrap();
        let resume = cmd.iter().position(|a| a == "resume").expect("resume");

        assert_eq!(cmd[resume + 1], "uuid-7");
        assert_eq!(cmd.last().unwrap(), "do the thing");
        assert_eq!(cmd.len(), resume + 3, "nothing may follow the Brief");
    }

    /// A bare `resume` opens codex's interactive session picker.
    #[test]
    fn a_fresh_codex_never_says_resume() {
        let cmd = build(spawn(Agent::Codex, None, None)).unwrap();
        assert!(!cmd.iter().any(|a| a == "resume"));
    }

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
            assert!(rendered.contains(r#"command=""#), "{rendered}");
            assert!(rendered.contains(&format!("'{subcommand}'")), "{rendered}");
            assert!(rendered.contains("--spawn-token 'tok-1'"), "{rendered}");
            assert!(rendered.contains("'/Applications/Tethys.app/tethys-hook'"));
        }
    }

    #[test]
    fn no_hook_binary_means_no_hook_overrides() {
        let cmd = build(spawn(Agent::Codex, None, None)).unwrap();
        assert!(!cmd.iter().any(|a| a.starts_with("hooks.")));
    }

    #[test]
    fn codex_is_granted_each_repos_git_dir() {
        let mut s = spawn(Agent::Codex, None, None);
        let dirs = vec![PathBuf::from("/data/repos/frontend.git")];
        s.extra_writable = &dirs;
        let cmd = build(s).unwrap();
        let i = cmd.iter().position(|a| a == "--add-dir").expect("--add-dir");
        assert_eq!(cmd[i + 1], "/data/repos/frontend.git");
    }

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

    #[test]
    fn toml_values_are_quoted_and_escaped() {
        assert_eq!(toml_string("plain"), r#""plain""#);
        assert_eq!(toml_string(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(toml_string(r"back\slash"), r#""back\\slash""#);
    }
}

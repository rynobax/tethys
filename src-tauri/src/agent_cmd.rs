//! The argv Tethys spawns an agent CLI with.
//!
//! One function per agent, because the two command lines have almost nothing
//! in common beyond the binary and the trailing prompt. Claude Code takes a
//! `--resume` flag and an inline `--mcp-config`, and reads its hooks from
//! `~/.claude/settings.json`. Codex resumes through a subcommand and takes
//! MCP servers, hooks and everything else as `-c` config overrides. Trying to
//! express both as one parameterised builder produces a shape that describes
//! neither.

use std::path::Path;

use crate::agent::Agent;
use crate::error::AppResult;
use crate::mcp::McpLaunch;

/// What a spawn needs to know to render a command line.
pub struct Spawn<'a> {
    pub agent: Agent,
    /// The already-resolved binary path.
    pub agent_bin: &'a Path,
    pub workspace_id: &'a str,
    /// Tethys's id for the session — also the tmux session name. Baked into
    /// the MCP config so a tool call can name its own session.
    pub session_id: &'a str,
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

fn codex(_spawn: Spawn<'_>) -> AppResult<Vec<String>> {
    Err(crate::error::AppError::Other(
        "codex sessions are not wired up yet".into(),
    ))
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
}

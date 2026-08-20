# Tethys

Desktop app for running multiple parallel Claude Code sessions across git worktrees. One person's tool: workspaces bundle worktrees and sessions, and Tethys owns the lifecycle around them.

## Language

### Core

**Repo**:
A git repository registered in `repos.toml`, identified by its stable `repo_key`. Tethys maintains its own clone of each repo and never touches checkouts the user manages separately.

**Workspace**:
A named unit of parallel work: one branch name shared across N repos, each checked out as a worktree, plus the Claude sessions running inside them.
_Avoid_: Project, sandbox

**Repo Link**:
A repo's membership in a workspace — the worktree path plus per-repo state like whether Tethys created the branch.

**Session**:
A Claude Code CLI process attached to a workspace, either inside one repo's worktree or at the workspace root.

**Notes**:
Freeform per-workspace text the user writes in the UI.

### Lifecycle

**Soft Delete**:
Marking a workspace deleted without touching disk. Reversible for the grace window; running scripts and tmux sessions are killed immediately.

**Purge**:
The background teardown that runs after the grace window: worktrees removed, Tethys-created branches deleted, workspace dropped from state.

**Pending Permissions**:
Workspace-local Claude permission grants captured at purge for the user to later fold into the shared per-repo settings or discard.

### Handoff

**Handoff**:
A workspace created by an agent from inside a running session, rather than by the user in the UI.

**Origin**:
Where a workspace came from — the user's own hand, or a handoff, and which session did the handing off.

**Brief**:
The instruction an agent writes for the session it hands work to. Unlike Notes, it is written by an agent, for an agent, and only at the moment of handoff.

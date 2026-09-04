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

### Folders

**Folder**:
A named place in the sidebar holding workspaces, so they can be grouped and collapsed out of the way. A workspace is in exactly one folder, folders don't nest, and being in one changes nothing about how a workspace behaves.
_Avoid_: Tag, label, group

**Default Folder**:
Where a workspace sits when the user hasn't filed it anywhere. Always present, and not named by the user the way the folders they create are. New workspaces land wherever the user picks; a handoff lands in the same folder as the session that asked for it.

### Blocking

**Blocker**:
The workspace another workspace is waiting on before its own work can continue. Always declared — by the user, or by an agent at handoff — never inferred from branch or PR topology.
_Avoid_: Dependency, upstream, parent

**Blocked**:
A workspace that has a blocker: at most one at a time, though one blocker can hold up several workspaces. It stops being blocked once its blocker leaves the sidebar, ends up in a different folder, or the link is cleared.

### Lifecycle

**Soft Delete**:
Marking a workspace deleted without touching disk. Reversible for the grace window; running scripts and tmux sessions are killed immediately.

**Purge**:
The background teardown that runs after the grace window: worktrees removed, Tethys-created branches deleted, workspace dropped from state.

**Setup Queue**:
The line workspaces wait in to be provisioned, because Tethys sets up one at a time. First asked for, first built — and a workspace waiting its turn is **Queued**: asked for, nothing on disk yet, nothing running.
_Avoid_: Job queue, build queue

**Pending Permissions**:
Workspace-local Claude permission grants captured at purge for the user to later fold into the shared per-repo settings or discard.

### Handoff

**Handoff**:
A workspace created by an agent from inside a running session, rather than by the user in the UI.

**Origin**:
Where a workspace came from — the user's own hand, or a handoff, and which session did the handing off.

**Brief**:
The instruction an agent writes for the session it hands work to. Unlike Notes, it is written by an agent, for an agent, and only at the moment of handoff.

### Pull requests

**Branch PR**:
The pull request open on a workspace's own branch. Tethys finds it by itself, by branch name, and there is at most one per repo link.

**Attached PR**:
Any other pull request a workspace should show — a stacked PR, a follow-up, something cut off main in the same worktree. Named by the user in the attach dialog or by an agent linking one it opened, since nothing about the workspace implies it.
_Avoid_: Extra PR, secondary PR

### Side panel

**Side Panel**:
The collapsible pane on the right of a workspace, holding its Notes and its Artifacts, one tab each.
_Avoid_: Inspector, drawer, right pane

**Artifact**:
Something a session produced that Tethys can show rather than leave as text in the terminal. Belongs to the workspace the session runs in, and kept only while Tethys is running.
_Avoid_: Output, attachment

**Diagram**:
An Artifact that is a Mermaid diagram, taken from the source a session wrote in its reply.

**Page**:
An Artifact that is an HTML file a session wrote inside the workspace, shown as the file currently on disk.

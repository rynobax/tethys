# Tethys

Desktop app for running multiple parallel agent CLI sessions across git worktrees. One person's tool: workspaces bundle worktrees and sessions, and Tethys owns the lifecycle around them.

## Language

### Core

**Repo**:
A git repository registered in `repos.toml`, identified by its stable `repo_key`. Tethys maintains its own clone of each repo and never touches checkouts the user manages separately.

**Workspace**:
A named unit of parallel work: one branch name shared across N repos, each checked out as a worktree, plus the one agent session running inside them.
_Avoid_: Project, sandbox

**Repo Link**:
A repo's membership in a workspace — the worktree path plus per-repo state like whether Tethys created the branch.

**Agent**:
Which agent CLI a workspace's session runs — Claude Code or codex. Chosen when the workspace is created, inherited by a handoff, and changeable afterwards, though changing it necessarily starts a new conversation. Distinct from the **binary**, the executable name the agent runs as (`claude-hipaa` is a Claude): the agent decides how a session is spawned, hooked and resumed, and is never inferred from the binary's spelling.
_Avoid_: Harness, model, provider

**Session**:
The one agent CLI process a workspace runs, at the workspace root. A workspace has at most one; starting, resuming and reconnecting it are the same act.

**Notes**:
Freeform per-workspace text the user writes in the UI.

### Folders

**Folder**:
A named place in the sidebar holding workspaces, so they can be grouped and collapsed out of the way. A workspace is in exactly one folder, folders don't nest, and being in one changes nothing about how a workspace behaves.
_Avoid_: Tag, label, group

**Default Folder**:
Where a workspace sits when the user hasn't filed it anywhere. Always present, and not named by the user the way the folders they create are. New workspaces land wherever the user picks; a handoff lands in the same folder as the session that asked for it.

### Children

**Child**:
A workspace an agent handed work to and asked to keep track of, drawn beneath the workspace that made it. When the user deletes it, that workspace's session is told, along with what became of its pull requests. Every child is a Handoff; not every Handoff is a child.
_Avoid_: Sub-workspace, blocker, dependent

**Parent**:
The workspace a child was handed out from. It can't be deleted while any of its children are still live, and it hears only about its own children, not theirs.

### Lifecycle

**Soft Delete**:
Marking a workspace deleted without touching disk. Reversible for the grace window; the session's tmux pane is killed immediately.

**Purge**:
The background teardown that runs after the grace window: worktrees removed, Tethys-created branches deleted, workspace dropped from state.

**Setup Queue**:
The line workspaces wait in to be provisioned, because Tethys sets up one at a time. First asked for, first built — and a workspace waiting its turn is **Queued**: asked for, nothing on disk yet, nothing running.
_Avoid_: Job queue, build queue

**Stray**:
A process a session started that outlived the session — a backgrounded dev server, an orphaned test run. Tethys finds strays by the session tag they inherited and kills them.
_Avoid_: Orphan, zombie, leak

**Memory Budget**:
How much memory everything a session started may hold at once, the agent CLI itself aside. A session over it loses its biggest job, and the agent is told why. The user can raise it for a while when an agent asks.
_Avoid_: Quota, memory limit, cap

### Handoff

**Handoff**:
A workspace created by an agent from inside a running session, rather than by the user in the UI.

**Origin**:
Where a workspace came from — the user's own hand, or a handoff, and which session did the handing off.

**Brief**:
The instruction an agent writes for the session it hands work to. Unlike Notes, it is written by an agent, for an agent, and only at the moment of handoff.

### Pull requests

**Tracked PR**:
A pull request a workspace shows and keeps polling, in one list per repo link. The PR on the workspace's own branch is found and added automatically; any other (a follow-up, something cut off main in the same worktree) is linked by an agent. Tracking ends only when the user detaches it.
_Avoid_: Branch PR, attached PR, extra PR

**Stack**:
Pull requests GitHub knows to be one `gh stack`, drawn together in base-first order. Tethys recognizes stacks and never creates or restacks them.
_Avoid_: Chain, PR train

### Shipping

**Shipping**:
The state the user puts a workspace with at least one Tracked PR in once its work is done and its Shipping PRs only need to get merged. It sits apart from the workspaces still being worked on, its session fixes what it can without the user, and it asks for the user's attention once every Shipping PR is approved and passing. It stays Shipping until its Shipping PRs are all closed, or until the user takes it out or the agent says it needs the user, either of which sends it back among the workspaces being worked on. Talking to its session doesn't change that. A workspace that isn't Shipping or Shipped has no special name.
_Avoid_: Landing, Outbox, Shippable, Ready, Parked

**Shipping PR**:
A Tracked PR the user named as the one being shipped when moving a workspace to Shipping. A workspace's only PR is its Shipping PR without asking. Its other Tracked PRs play no part in Shipping, and being in a Stack makes no difference.

**Nudge**:
A message Tethys sends a Shipping workspace's session telling it to fix something on its own, such as a merge conflict, failing CI or a merge-queue failure.

**Shipped**:
What a Shipping workspace becomes once all its Shipping PRs are closed, merged or not: still shown with the Shipping ones, marked done, waiting for the user to delete it. A workspace with any other PR still open goes back among the workspaces being worked on instead.

### Workspace tabs

**Tab**:
One full-pane view of a ready workspace: the Chat (the session's terminal), one per Tracked PR, and one per Artifact.

**Side Panel**:
The collapsible pane beside the Chat that holds the workspace's Notes.
_Avoid_: Inspector, drawer, right pane

**Artifact**:
Something a session produced that Tethys can show rather than leave as text in the terminal. Belongs to the workspace the session runs in, and kept with it until closed or crowded out by newer ones.
_Avoid_: Output, attachment

**Diagram**:
An Artifact that is a Mermaid diagram, taken from the source a session wrote in its reply.

**Page**:
An Artifact that is an HTML file a session wrote inside the workspace, shown as the file currently on disk.

**Review**:
An Artifact that is an ummon review a session started on one of the workspace's repos.

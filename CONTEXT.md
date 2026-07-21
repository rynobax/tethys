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
Freeform per-workspace text the user writes in the UI. Unrelated to Managed Docs.

### Managed Docs

**Managed Docs**:
Per-repo `CONTEXT.md` and ADRs that the user's team hasn't adopted, so they can't live in the repo itself. Tethys stores them, surfaces them in every workspace, and folds changes back together.

**Docs Repo**:
The Tethys-owned git repository holding one repo's Managed Docs. Its `main` branch is the canonical version; per-workspace branches diverge from it.

**Docs Checkout**:
A workspace's private git worktree of a Docs Repo. Its files are symlinked into the user-repo worktree (only paths that exist — never dangling links) and hidden from the user repo's git status.

**Snapshot**:
The single commit made to a workspace's docs branch when the workspace is purged, capturing everything the workspace changed. Managed Docs have no mid-life commits.

**Adoption**:
Snapshot-time capture of real `CONTEXT.md` / `docs/adr` files a session created directly in the worktree because they didn't yet exist in the Docs Repo to symlink.

**Pending Docs Merge**:
A parked docs branch whose Snapshot differs from docs main, awaiting an explicit approve (merge) or decline (archive). Identical branches are deleted silently; a conflicting approve parks it as conflicted for manual resolution.
_Avoid_: Auto-merge

**Team-Adopted**:
A repo whose own git history tracks `CONTEXT.md` or `docs/adr`. Managed Docs are disabled for that path — the team's version is canonical and Tethys never shadows it.

### Lifecycle

**Soft Delete**:
Marking a workspace deleted without touching disk. Reversible for the grace window; running scripts and tmux sessions are killed immediately.

**Purge**:
The background teardown that runs after the grace window: worktrees removed, Tethys-created branches deleted, docs Snapshot taken, workspace dropped from state.

**Pending Permissions**:
Workspace-local Claude permission grants captured at purge for the user to later fold into the shared per-repo settings or discard. The pattern Pending Docs Merges follow.

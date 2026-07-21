# Managed Docs use a git branch per workspace, not shared live symlinks

Managed Docs (per-repo CONTEXT.md + ADRs the team hasn't adopted) must persist across workspaces, survive workspace deletion with a review step, and be recoverable after mistakes. We store them in a Tethys-owned Docs Repo per `repo_key` and give each workspace its own branch and Docs Checkout, symlinked into the user-repo worktree. Deletion snapshots the branch; any diff against docs main parks as a Pending Docs Merge for explicit approve/decline.

## Considered Options

- **Shared live symlinks** (the `settings.local.json` pattern): one copy per repo, every worktree links to it. Rejected because parallel Claude sessions in different workspaces would race on the same file (last write wins), and there is nothing to review on deletion — the user explicitly wants to approve or decline a workspace's docs changes before they become canonical.
- **Copy in / diff out** (the `copy_files` + pending-permissions pattern): no git machinery, but merging concurrent edits from two workspaces degrades to crude whole-file diffs. Git gives real 3-way merges and free history.

## Consequences

- Tethys runs git against its own state for the first time (everything else is plain JSON/TOML).
- Docs branches commit only at purge (Snapshot). A mid-life mistake in a long-lived workspace has no intermediate history to recover from — accepted trade-off.
- Symlinked paths are hidden via the clone's shared `.git/info/exclude`, so they never appear in the user repo's git status in any workspace.
- If the user repo itself starts tracking CONTEXT.md or docs/adr (Team-Adopted), Managed Docs for that path are disabled rather than shadowing the team's file.

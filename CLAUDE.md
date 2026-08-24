# Tethys

Desktop app for managing multiple Claude Code CLI sessions in parallel across git worktrees. Each "workspace" bundles N worktrees (one per repo) plus the Claude sessions running inside them, with "your turn" notifications driven by Claude Code hooks.

**This is a personal tool built for Ryan.** No multi-user, no cross-platform, no distribution plans. macOS-only for the foreseeable future — feel free to take macOS-specific paths, shell invocations, or Tauri features without guarding them.

This repo has no branches — work directly on `main` and commit there. Don't create feature branches or PRs.

## Stack

Tauri 2.x shell · Rust core (`src-tauri/`) · React + TypeScript frontend (`src/`) · xterm.js (DOM renderer) for terminal rendering · `portable-pty` for PTY spawning · JSON file persistence (no SQLite) · `tethys-hook` companion binary (`crates/tethys-hook/`) that forwards Claude Code hooks over a Unix socket · `tethys-mcp` companion binary (`crates/tethys-mcp/`, built on the `rmcp` SDK) that gives each session two MCP tools: handing work off to a new workspace, and linking a PR to the current one.

## Running

```
pnpm tauri dev
```

State lives at `~/Library/Application Support/app.tethys.dev/` (`state.json`, `logs/`, `repos.toml`, auto-generated `repos.schema.json`, `hook.sock`, `mcp.sock`).

Tethys writes its hook entries into `~/.claude/settings.json` on every boot (keyed by `description: "Tethys session monitor"`). They're idempotent — safe to leave across reinstalls.

It also generates a `CLAUDE.md` at each workspace root (`workspace_doc.rs`) explaining the worktree layout and telling sessions to ask for a missing repo rather than reading some other checkout. Rewritten on create, on repo-add, and at every boot. Claude Code reads CLAUDE.md from every parent dir, so the root file also applies to per-repo sessions.

The prose lives in `repos.toml`, not in Rust: `[workspace_doc].body` (with `{branch}` / `{repo_list}` / `{available_repos}` / `{workspace_root}` / `{clone_dir}` placeholders) falling back to `DEFAULT_BODY`, plus per-repo `claude_notes`. Rust owns only the marker line and the "Repo notes" section.

## The session's MCP tools

Tethys renders a `--mcp-config` per session at spawn time (inline JSON, nothing on disk) pointing at `tethys-mcp`, and pairs it with `--allowed-tools` naming *every* tool, so no call can stall on a permission dialog nobody is watching. Never `--strict-mcp-config`, which would cut the session off from every other MCP server. Two tools live behind it, sharing one socket and told apart by the frame's `op` tag.

### Handoffs

A session can create a *new* workspace — a **handoff** — through `mcp__tethys__create_workspace`.

The calling workspace and session ids are baked into that config's `env` block, not passed as tool arguments — that's what makes the recorded `Origin` unforgeable. The new workspace inherits the caller's `claude_binary`, so work can't drift out of a `claude-hipaa` workspace by accident.

The tool returns as soon as the `Creating` draft is in state: provisioning runs in the background and the calling agent never hears how it went, so a failure shows up only as a `CreationFailed` row in the UI. Every Tethys-spawned session gets the tool, including handoff-created ones — chains are possible and there is no concurrency cap yet.

### Linking a PR

`mcp__tethys__link_pr` points the *calling* workspace at a PR that already exists on GitHub — the agent-facing half of the attach dialog, and the same code (`github/attach.rs`) behind both. It takes a reference (`123`, `#123`, `owner/repo#123`, or a URL) and an optional `repo_key` for when the workspace spans several GitHub repos; the workspace itself comes from the config's `env` block, like the handoff origin, so an agent can only ever link to its own.

The one judgement call in there is *which* of a repo link's two PR slots the PR lands in. `link.github` is the PR for the workspace's own branch, which the poller finds on its own; `attached_prs` is everything else. `record()` routes on `head_branch`, so an agent linking the PR it just opened for its own branch fills the branch slot rather than creating a second chip the poller would duplicate a tick later. That path is idempotent, and sweeps up any stale `attached_prs` copy of the same number.

Deliberately absent: no creating or editing PRs (this records, it doesn't act on GitHub), no unlinking, and no linking to any workspace but the caller's.

One trap worth remembering: Claude Code negotiates MCP protocol `2026-07-28`, which requires `ttlMs` on a `tools/list` reply. `rmcp`'s `with_all_items` omits it, and a reply without it is silently rejected and retried until the client reports "tools fetch failed". `tools_result()` sets it, and a test in `crates/tethys-mcp` holds it there.

## Folders

The sidebar is a partition: every workspace sits in exactly one folder, and `folder: None` *is* the **Default** folder — deliberately not a `Folder` at all, which is what makes it always present, unnameable, and impossible to delete or drag off the top. Folders are flat and inert: membership decides where a row is drawn and nothing else. Create none and the sidebar looks exactly as it did before they existed — headers appear only once a real folder does.

They replaced an `archived_at` marker that was the same idea wearing behaviour it didn't need. Archiving used to mute "your turn" notifications, drop the row from keyboard navigation, and suppress the "ready to delete" banner; none of that survived, so a workspace in a folder named Archived pings like any other. `Store::load` migrates any `archived_at` still in the file into an ordinary (collapsed) folder called "Archived", reading it off a `serde_json::Value` rather than the struct so `Workspace` carries no trace of the retired concept. The first flush without the field ends the migration for good.

Ordering is two Vecs — `AppState.folders` and the existing `AppState.workspaces`. Rendering re-groups the flat workspace order by folder, so only *relative* order within a folder can be observed; the drag handler leans on that, which is why its index math doesn't bother producing a globally tidy list.

Drag does all the moving. Headers drag to reorder folders; a workspace drops between rows or onto a header, where it appends — the only target a collapsed or empty folder can offer. Grabbing any row of a blocker stack moves the whole stack, so dragging can't split a blocker from its dependents. Deleting a folder sends its contents to Default rather than refusing or destroying anything. `move_workspaces_to_folder` and `reorder_folders` emit no events, for the same reason `reorder_workspaces` doesn't: the sidebar has already drawn the result. Folders are only ever written from the UI, so `App` mirrors each mutation into local state instead of round-tripping.

Deliberately absent: nesting, multi-membership, any agent-facing folder argument (a handoff inherits the caller's folder, like the binary), and any per-folder behaviour at all — including a rollup badge on a collapsed header, so a session wanting attention inside one is genuinely out of sight.

## Blockers

A workspace can be marked as waiting on another — its **blocker** — and the sidebar draws it inset beneath that row, joined by an elbow. Display only: nothing is paused, gated, or notified.

The link is a single `blocked_by: Option<WorkspaceId>` on `Workspace`. Everything else is derived. `workspaceTree` (`src/workspaceDerived.ts`) is the only place that decides whether a workspace *counts* as blocked, and its answer is "my blocker is one of the rows I'm drawn with" — it runs once per folder, so soft-deleting a blocker *or* moving it to another folder un-nests its dependents without touching a field, and undoing either brings the nesting back. Same-folder is enforced at both ends, in `blockerCandidates` and in `set_workspace_blocker`: a cross-folder link would be stored and then never drawn. The field is erased only where the id stops meaning anything: purge, `forget_workspace`, and the boot-time prune of non-`Ready` drafts in `Store::load`. Miss that last one and an agent-created blocker leaves a dangling pointer after a restart.

Cardinality falls out of the field's shape rather than being enforced: fan-in is capped at one because it's a single `Option`, and fan-out is free because the blocker is the *parent* row, so several dependents are just several children. Cycles are the one real invariant — `AppState::blocker_would_cycle` walks up the chain with a hop limit, because `state.json` is hand-editable and a file already carrying a cycle has to be survivable rather than hang the walk.

Ordering stays a single source of truth. `reorder_workspaces` still takes a flat id list, and the sidebar sends the whole *visual* order after a drag, not just the roots — so a workspace that later stops being blocked stays where it already appeared. Dragging moves a blocker together with everything nested under it; nested rows aren't draggable, and re-pointing is context-menu only.

Agents get at this through one extra `create_workspace` argument, `blocks_caller`. It's why drafts have to be legal blockers: the tool returns while the new workspace is still `Creating`, so the caller points at a row that won't finish provisioning for minutes. It overwrites any blocker already set.

Deliberately absent: no derivation from PR base branches (`baseRefName` is still not fetched), no notification when a blocker clears, no coupling to PR state, and no second blocker.

## Logging & diagnostics

Two log sinks, filtered independently (`logging.rs`):

- **File** — `logs/tethys.log.<date>`, full `info,tethys_lib=debug`. This is the real log.
- **stderr** — mirrored into whichever terminal ran `pnpm tauri dev`. Defaults to `warn` only, because it's an unbounded pipe into a terminal emulator's scrollback. `TETHYS_LOG_STDERR` overrides it (`off` to silence, `info,tethys_lib=debug` for the full firehose). `RUST_LOG` sets overall verbosity and caps *both* sinks — raising `TETHYS_LOG_STDERR` past it does nothing.

### Memory watchdog

`~/.local/bin/memwatch.sh` (outside this repo) samples system + per-app memory every 20s into `~/memwatch/samples.tsv`, and dumps `~/memwatch/snap-<ts>.txt` when iTerm2 >1.2GB or VS Code >5GB. Snapshots carry top-30 RSS, every tty and its command, `vmmap -summary` for iTerm2, and a tethys log tail. Start with `nohup memwatch.sh 20 &`, stop with `pkill -f memwatch.sh`.

Written to chase an intermittent "iTerm2 eats all the RAM" report. Findings from 8.5k samples over 5 days (2026-08-13 → 08-18):

- **iTerm2 was never the problem** — max 225MB, mean 111MB. Never came close to tripping.
- **Tethys is not a heavy process** — mean 66MB, and its stderr output measured ~236 KB/hour before the `warn` default landed. (One 770MB sample is the script catching a concurrent `cargo` build under `target/`, not the app.)
- **The machine is chronically oversubscribed** on 24GB: swap mean 7.1GB / max 13.3GB, compressor mean 7GB, free pages routinely <100MB. Chrome is the largest consumer (mean 4.0GB, max 7.2GB).
- **The one trip was VS Code**, 5.3GB at 2026-08-14T14:42 — a burst of Code renderer + extension-host processes <15s old, alongside `oxlint --lsp` running inside a Tethys worktree (`~/code/worktrees/<workspace>/nl-frontend`). Each worktree is a full checkout with its own `node_modules`, so opening several in VS Code multiplies the LSP/TS-server stack with nothing shared.

So: apparent app-level memory blowups here are most likely symptoms of system-wide pressure, not a leak in Tethys. Check `samples.tsv` before assuming otherwise.

## Rust

Use idiomatic rust. After a set of changes are finished, run clippy and clean up the issues it reports

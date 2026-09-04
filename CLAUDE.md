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

`record()` is a plain insert-or-refresh into the link's one PR list (see **Tracked PRs**), so linking a PR the poller already found is a refresh rather than a second chip. It reports back whether the PR turned out to be the one for the workspace's own branch — read off `head_branch`, not off where it was stored — so an agent can tell "I linked the PR I just opened" from "I linked somebody else's".

Deliberately absent: no creating or editing PRs (this records, it doesn't act on GitHub), no unlinking, and no linking to any workspace but the caller's.

One trap worth remembering: Claude Code negotiates MCP protocol `2026-07-28`, which requires `ttlMs` on a `tools/list` reply. `rmcp`'s `with_all_items` omits it, and a reply without it is silently rejected and retried until the client reports "tools fetch failed". `tools_result()` sets it, and a test in `crates/tethys-mcp` holds it there.

## The setup queue

Provisioning runs **one workspace at a time** (`provision_queue.rs`). Asking for five at once used to start five clones and five `pnpm install`s on one machine, and a setup script that takes two minutes alone can run past its `setup_timeout_secs` when it's sharing the disk with four others — which isn't a slow workspace but a failed one, since a timeout rolls the whole thing back.

The gate is a one-permit tokio semaphore, so admission is FIFO: the first workspace you asked for is the first you can start working in, rather than all five landing together at the end. Every path that provisions takes it — the create dialog, adding a repo to an existing workspace, and a handoff — which is why it's shared state rather than a field on any one of them. It is not a tuning knob; a concurrency budget is a different feature.

A job that has to wait says so twice: a status line on its job channel, and `WorkspaceStatus::Queued` on its own row, so a sidebar full of pending rows can say which one the machine is actually building. `Queued` is a draft like `Creating` — same boot-time prune, no worktrees, nothing running.

Waiting also made "deleted while creating" an ordinary thing to do rather than a race, so `provision_workspace` re-checks `deleted_at` the moment it gets the slot and abandons the job before the first clone. The row is left `Queued`, not `CreationFailed`: it never failed, it was called off.

Deliberately absent: no persisted queue (quit with four waiting and they're gone, exactly like a workspace caught mid-provision), no priority, no way to reorder or cancel a queued job except by deleting the workspace, and no cap on how many can wait.

## Folders

The sidebar is a partition: every workspace sits in exactly one folder, and `folder: None` *is* the **Default** folder — deliberately not a `Folder` at all, which is what makes it always present, unnameable, and impossible to delete or drag off the top. Folders are flat and inert: membership decides where a row is drawn and nothing else. Create none and the sidebar looks exactly as it did before they existed — headers appear only once a real folder does.

They replaced an `archived_at` marker that was the same idea wearing behaviour it didn't need. Archiving used to mute "your turn" notifications and drop the row from keyboard navigation; neither survived, so a workspace in a folder named Archived pings like any other. `Store::load` migrates any `archived_at` still in the file into an ordinary (collapsed) folder called "Archived", reading it off a `serde_json::Value` rather than the struct so `Workspace` carries no trace of the retired concept. The first flush without the field ends the migration for good.

Ordering is two Vecs — `AppState.folders` and the existing `AppState.workspaces`. Rendering re-groups the flat workspace order by folder, so only *relative* order within a folder can be observed; the drag handler leans on that, which is why its index math doesn't bother producing a globally tidy list.

Drag does all the moving. Headers drag to reorder folders; a workspace drops between rows or onto a header, where it appends — the only target a collapsed or empty folder can offer. Grabbing any row of a blocker stack moves the whole stack, so dragging can't split a blocker from its dependents. Deleting a folder sends its contents to Default rather than refusing or destroying anything. `move_workspaces_to_folder` and `reorder_folders` emit no events, for the same reason `reorder_workspaces` doesn't: the sidebar has already drawn the result. Folders are only ever written from the UI, so `App` mirrors each mutation into local state instead of round-tripping.

Deliberately absent: nesting, multi-membership, any agent-facing folder argument (a handoff inherits the caller's folder, like the binary), and any per-folder behaviour at all — including a rollup badge on a collapsed header, so a session wanting attention inside one is genuinely out of sight.

## Blockers

A workspace can be marked as waiting on another — its **blocker** — and the sidebar draws it inset beneath that row, joined by an elbow. Display only: nothing is paused, gated, or notified.

The link is a single `blocked_by: Option<WorkspaceId>` on `Workspace`. Everything else is derived. `workspaceTree` (`src/workspaceDerived.ts`) is the only place that decides whether a workspace *counts* as blocked, and its answer is "my blocker is one of the rows I'm drawn with" — it runs once per folder, so soft-deleting a blocker *or* moving it to another folder un-nests its dependents without touching a field, and undoing either brings the nesting back. Same-folder is enforced at both ends, in `blockerCandidates` and in `set_workspace_blocker`: a cross-folder link would be stored and then never drawn. The field is erased only where the id stops meaning anything: purge, `forget_workspace`, and the boot-time prune of non-`Ready` drafts in `Store::load`. Miss that last one and an agent-created blocker leaves a dangling pointer after a restart.

Cardinality falls out of the field's shape rather than being enforced: fan-in is capped at one because it's a single `Option`, and fan-out is free because the blocker is the *parent* row, so several dependents are just several children. Cycles are the one real invariant — `AppState::blocker_would_cycle` walks up the chain with a hop limit, because `state.json` is hand-editable and a file already carrying a cycle has to be survivable rather than hang the walk.

Ordering stays a single source of truth. `reorder_workspaces` still takes a flat id list, and the sidebar sends the whole *visual* order after a drag, not just the roots — so a workspace that later stops being blocked stays where it already appeared. Dragging moves a blocker together with everything nested under it; nested rows aren't draggable, and re-pointing is context-menu only.

Agents get at this through one extra `create_workspace` argument, `blocks_caller`. It's why drafts have to be legal blockers: the tool returns while the new workspace is still `Creating`, so the caller points at a row that won't finish provisioning for minutes. It overwrites any blocker already set.

Deliberately absent: no derivation from PR bases or `gh stack` membership — a stack is drawn inside one workspace's header and never nests one workspace under another — no notification when a blocker clears, no coupling to PR state, and no second blocker.

## Tracked PRs

A repo link tracks **one list** of PRs (`RepoLink::prs`). The PR for the workspace's own branch is in there like any other; the only thing special about it is that it gets *added* for you.

It used to have a slot of its own, `link.github`, with `attached_prs` beside it — and that split quietly bought the branch PR four behaviours nothing else had: it couldn't be detached, it re-derived itself from the branch every tick, it vanished silently where an attached PR showed "no data", and re-pointing at it was a refresh where re-attaching anything else was an error. Every one of those was an accident of storage rather than a decision, so the list replaced them. `Store::load` folds both old fields in (`migrate_pr_slots`), reading them off a `serde_json::Value` so `RepoLink` carries no trace of the split, and the first flush ends the migration.

The poller's two target kinds are now the whole of the asymmetry, and they're a division of labour rather than of privilege. `TargetKind::Branch` is a *scan*: it selects only `number` and answers "which PR should this link be tracking", which is the automatic equivalent of typing a number into the attach dialog. `TargetKind::Pr` pulls the full selection and is what every tracked PR is polled with afterwards, no matter how it got there. So a tick is two passes — scan, then fetch anything the scan just started tracking — because attaching by hand fetches before it records, and a PR you just opened shouldn't sit there saying "no data" for 45 seconds when a hand-attached one wouldn't.

Two consequences worth knowing, both of them the point rather than side effects:

- **A scan takes nothing away.** Tracking ends at detach, not at whatever the branch currently points to. Close the PR on your branch and open another, and you get two chips instead of one silently swapped for the other.
- **Detach works on everything**, which is why `untrack` records the number in `dismissed`: a scan runs every tick and would otherwise put the branch's PR straight back, so detaching it would last 45 seconds. Attaching a dismissed number clears it — asking for a PR by name outranks having once said no to it.

Gone with the split: `isReadyToDelete` and its "ready to delete" banner. It asked whether every tracked PR was merged, which was only ever answerable because the branch slot re-derived itself; with tracking that persists, a closed-and-replaced PR would have pinned it false until someone detached the corpse. Nobody was using the banner to decide anything.

## PR stacks

The members of a **`gh stack`** are drawn wrapped in one outline in the workspace header, base-first, separated by a chevron. Each PR keeps its own chip — the container is the whole of what stacking adds. Ryan builds stacks with `gh stack` (`github/gh-stack`), so Tethys only has to recognize them, never create or restack them.

Membership is GitHub's own, not inferred. `github/gh-stack` makes a stack a real object on GitHub's side, and GraphQL exposes it on `PullRequest` as `stack { number size }` plus `stackEntry { position }` (1 is closest to the base branch) — all added to the poller's `PR_FIELDS` and flattened into one `Option<PrStack>` on `GithubPrStatus`. `parse_stack` needs all three numbers or yields `None`: a stack we can't place the PR within groups chips it can't order.

Chaining PRs by base branch instead was the obvious cheaper route and is wrong. A hand-made chain and a `gh stack` are indistinguishable from `baseRefName` alone, and only one of them is a thing you can `gh stack sync` — so grouping on bases would put an outline around PRs that have no stack to manage. Nothing fetches `baseRefName`.

`prGroups` (`src/workspaceDerived.ts`) partitions a link's PRs by stack number, ordering each group by `position`, and hands back every PR exactly once with non-stacked ones as groups of one — so the header renders groups uniformly and a workspace with no stack looks exactly as it did before. Every PR the link tracks feeds in, however it got there, since a stack's other branches are ones you attached by hand from this workspace's point of view. Stack numbers are per repository and a link is one repo, so nothing can pull two repos' PRs into one group.

A group is often *smaller* than `stack.size`: six stacked branches with one checked out here is one chip. That case still gets the outline — the PR is genuinely stacked and a bare chip would hide it — plus a `1 of 6` marker, which is the only thing distinguishing "this is the stack" from "this is what I have of it".

Deliberately absent: no stacks in the sidebar (it keeps its flat chip list, so a stack is only visible once you open the workspace), no rollup of the stack's CI or review state onto the container, no `gh stack` invocation of any kind, no fetching the stack-mates the workspace doesn't already track, and no link between a stack and the blocker nesting — separate ideas that happen to look similar.

## Side panel

Every ready workspace has a **Side Panel** on the right (`SidePanel.tsx`): its Notes and its **Artifacts**, one tab each. An artifact is something a session produced that Tethys can show rather than leave as terminal text — a **Diagram** (mermaid source) or a **Page** (an HTML file). Collapsed, it's a 28px rail that is itself the button; width and collapsed state are UI chrome, so they live in `localStorage` and apply to every workspace. Notes used to be a floating overlay with its own header button and an auto-open-when-non-empty rule; both are gone, and the rail's dot is what says a workspace has notes.

Artifacts come from **hooks, not from the screen** (`artifacts.rs`). Scraping xterm's buffer was the obvious route and is the wrong one: Claude's renderer uses the alternate screen, so only the visible fraction of a diagram is ever in the buffer, long lines wrap, and a half-streamed diagram is a parse error on every keystroke. The `Stop` hook already carries `last_assistant_message` — the exact markdown, complete — so a Diagram is a fence scan over that. A Page is `PostToolUse` for `Write`/`Edit`/`MultiEdit` on a `.html` path, *or* a Bash command that `open`s one — `tethys-hook` forwards `tool_name`, `tool_file_path` and `tool_command` for this, the only fields of any tool's input Tethys reads. The Bash case is there because the very first page a session made for this was a heredoc, invisible to the file tools; `/show-me` ends every page with `open path/to/show-me-*.html`, so that's the signal that catches the file however it was written. Relative paths resolve against the hook's `cwd`, and every page path is canonicalized so the `Write` and the `open` of one file are one tab. Both reuse the session→workspace correlation the turn tracker uses (`resolve_session`), so a page a subagent writes lands on its parent's workspace for free.

Two limits worth knowing. `last_assistant_message` is only the turn's *final* text block, so a diagram drawn before a tool call in the same turn is missed — `record_diagrams` logs at debug on a Stop with no fences, which is how we'd learn that matters. And a Page must resolve under the workspace root (`reconcile::is_under`), so a write to `/tmp` or another checkout can't put a tab in this workspace; an `.html` written via a heredoc and never `open`ed is invisible.

Seeing the same thing again is a **bump, not a new tab**: same page path or identical diagram source moves the existing artifact to the newest position and increments `revision`, which is what the Page iframe is keyed on so a re-edited page reloads in place. Artifacts live on `Workspace::artifacts` and so persist in `state.json` — they started out in memory, and the first design session that spanned a restart showed why that was wrong. The cap of 12 per workspace, oldest evicted, is what keeps the file from becoming a graveyard; `Store::load` also drops any Page whose file has gone. Writes go through `update_workspace_quiet`, because `artifact:changed` is the panel's own signal — it carries which artifact to select, and doesn't make the sidebar refetch every workspace.

The one thing that overrides your collapsed/expanded choice is a fresh artifact for the workspace you're looking at: the panel expands and selects it, because a `/show-me` turn is one where you want the screen taken. Arrivals for other workspaces just accumulate. Tab labels for a Diagram come from a `title` in the source, else the heading or `**bold**` lead-in just above the fence, else the diagram keyword — cheap heuristics whose fallback is dull rather than wrong.

Pages render in a sandboxed iframe (`allow-scripts`, no `allow-same-origin`) over Tauri's asset protocol, which is enabled with an *empty* static scope and opened at boot to `worktree_root` only (`lib.rs`), since that's runtime config from `repos.toml`. "Open in browser" goes through a Rust command rather than plugin-opener's `openPath`, which is scope-gated to paths fixed in the capability file. `mermaid` is a couple of megabytes and is imported on first use; a diagram that doesn't parse shows its source and the parser's complaint rather than nothing.

Deliberately absent: keyboard shortcuts, close-all, zoom/pan on diagrams, filtering by session, reading the transcript for earlier text blocks, and an `open` shim on the session's `PATH` — feasible, since Tethys owns the spawn env, but it would shadow a system command for every subprocess in the session to save the browser tab the hook already lets us observe.

## Logging & diagnostics

Two log sinks, filtered independently (`logging.rs`):

- **File** — `logs/tethys.log.<date>`, full `info,tethys_lib=debug`. This is the real log.
- **stderr** — mirrored into whichever terminal ran `pnpm tauri dev`. Defaults to `warn` only, because it's an unbounded pipe into a terminal emulator's scrollback. `TETHYS_LOG_STDERR` overrides it (`off` to silence, `info,tethys_lib=debug` for the full firehose). `RUST_LOG` sets overall verbosity and caps *both* sinks — raising `TETHYS_LOG_STDERR` past it does nothing.

### Memory watchdog

`scripts/memwatch.sh` samples system + per-app memory every 20s into `~/memwatch/samples.tsv`, and dumps `~/memwatch/snap-<ts>.txt` when a threshold trips. **Tethys launches it at boot** (`memwatch.rs`) — it was never running when a hang happened otherwise. `TETHYS_MEMWATCH=off` disables it, an integer overrides the interval. `~/.local/bin/memwatch.sh` is a symlink to the repo copy.

Tethys owns only the *start*. The script is a singleton on a pidfile, and detaches into a subshell ignoring `SIGHUP`/`SIGINT`, so it survives both the `pnpm tauri dev` that launched it and Tethys itself — Tethys is one of the suspects, and a watchdog that dies with the suspect cannot record the aftermath. Stop it with `pkill -f memwatch.sh`.

Memory is **`phys_footprint`** — the number Activity Monitor shows — read from one `top -l 1` per sample and joined against `ps` for full argv, so every column in a row describes one instant. The original script used `ps rss`, which excludes compressed and swapped pages and so understates by 2-3x *exactly* under the pressure worth catching.

Columns worth knowing: `claude_mb` counts the session processes, `claude_kids_mb` everything they spawned (test runners — the thing usually costing the memory); `tethys_mb` is the Rust binary tree, `webview_mb` the `WebKit.WebContent` process where a Tauri app's UI memory actually lives, `devstack_mb` the `pnpm tauri dev` tree minus Tethys; `dockervm_mb` catches `docker compose` test runs, which land in a VM descended from no session. Trips are on *system pressure* — any single process >8GB, swap >12GB, free <250MB — not on a named app.

The pre-2026-08-31 findings below came from the RSS-based script and understate every figure. Treat the iTerm2 line as unproven rather than settled: it was measured with the wrong instrument.

- **iTerm2 was never the problem** — max 225MB, mean 111MB (RSS; footprint measures ~280MB idle).
- **Tethys is not a heavy process** — mean 66MB, and its stderr output measured ~236 KB/hour before the `warn` default landed.
- **The machine is chronically oversubscribed** on 24GB: swap mean 7.1GB / max 13.3GB, compressor mean 7GB, free pages routinely <100MB. Chrome is the largest consumer (mean 4.0GB, max 7.2GB).
- **The one trip was VS Code**, 5.3GB at 2026-08-14T14:42, alongside `oxlint --lsp` in a Tethys worktree. Each worktree is a full checkout with its own `node_modules`, so opening several in VS Code multiplies the LSP/TS-server stack with nothing shared.

First minute of the rewritten script contradicts the "nothing here is big" reading: `claude_kids_mb` hit 11.2GB while `dockervm_mb` sat at 6.8GB and Chrome at 6.4GB — ~30GB of footprint on a 24GB machine. Sessions running test suites are a live suspect; check `claude_kids_mb` and `dockervm_mb` before blaming Tethys.

## Rust

Use idiomatic rust. After a set of changes are finished, run clippy and clean up the issues it reports

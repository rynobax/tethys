# Tethys

Desktop app for running many Claude Code (or codex) sessions in parallel across git worktrees. A **workspace** is N worktrees, one per enabled repo, plus the one agent session running at their root. Hooks drive its "your turn" state.

A personal tool for Ryan: macOS only, single user, never distributed. Take macOS-specific paths and Electron features without guarding them. Work and commit directly on `main`; no branches or PRs.

## Stack

Electron shell (`electron/`) · Rust core (`src-tauri/`) running as the headless `tethysd` · React + TypeScript (`src/`) · xterm.js over tmux-backed PTYs · JSON persistence · `crates/tethys-hook` forwards agent hooks over a Unix socket · `crates/tethys-mcp` serves each session's `mcp__tethys__*` tools.

## Running

`pnpm dev` builds the Rust binaries, serves the UI on port 1430 and launches Electron, which spawns `tethysd`. `pnpm install:app` packages it and replaces `/Applications/Tethys.app`.

State is in `~/Library/Application Support/app.tethys.dev/` (`state.json`, `repos.toml`, `logs/`, sockets). Electron's profile, which holds the GitHub login, is in `~/Library/Application Support/Tethys/`.

The daemon won't start while another Tethys is running. To run one beside it, set `TETHYS_DATA_DIR=$(mktemp -d)` (`docs/diagnostics.md`); add `TETHYS_TEST=1` to keep the window hidden.

Boot rewrites Tethys's hook entries in `~/.claude/settings.json` (idempotent) and each workspace root's generated `CLAUDE.md` (`workspace_doc.rs`, prose in `repos.toml`'s `[workspace_doc]`), which a save in Settings also does.

## After a change

- Rust: run clippy and fix what it reports.
- Frontend: `pnpm format` (prettier defaults; `.prettierrc.json` is empty on purpose). `pnpm build` runs the IPC parity check, tsc for `src/` and `electron/`, and both bundles.

## Conventions

- Old `state.json` shapes are migrated in `Store::load`, read off a `serde_json::Value` so structs carry no retired fields. The first flush ends the migration.
- MCP tools take the calling workspace from the server's `env` block, never from arguments, so an agent can only act on its own workspace.
- Reactions to PR status (Nudges, auto-ship) are edge-triggered: they fire when a status changes, not while it holds.
- No agent abstraction and no renderer interface for fun mode. Each has a sample of two at most; don't add a trait until a third exists.
- `repos.toml` changes while Tethys runs: Settings writes it (through `toml_edit`, keeping the user's comments) and swaps the `Registry`. Take one `registry.get()` snapshot per operation and hold it, never re-read mid-way. A spare for the wrong repos or root is rebuilt; anything else a create recomputes.
- `folder: None` is the Default folder, deliberately not a `Folder`. Folders only decide where a row is drawn.

## Design notes

Read the one for the area you're changing. They hold the reasons and traps the code can't show.

| Area | Doc |
|---|---|
| The Electron shell, the daemon and the socket between them | `docs/shell.md` |
| Session lifecycle, strays, memory budget | `docs/sessions.md` |
| Claude vs codex, the codex command line, terminal replay | `docs/agents.md` |
| `mcp__tethys__*` tools | `docs/mcp-tools.md` |
| Setup queue, the spare, per-worktree env | `docs/provisioning.md` |
| Child workspaces and the session inbox | `docs/children.md` |
| Tracked PRs, stacks, Shipping | `docs/prs.md` |
| Workspace tabs, artifacts, the PR webview | `docs/workspace-tabs.md` |
| Fun mode and its screenshot script | `docs/fun-mode.md` |
| Logs and the memory watchdog | `docs/diagnostics.md` |

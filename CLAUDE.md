# Tethys

Desktop app for managing multiple Claude Code CLI sessions in parallel across git worktrees. Each "workspace" bundles N worktrees (one per repo) plus the Claude sessions running inside them, with "your turn" notifications driven by Claude Code hooks.

**This is a personal tool built for Ryan.** No multi-user, no cross-platform, no distribution plans. macOS-only for the foreseeable future — feel free to take macOS-specific paths, shell invocations, or Tauri features without guarding them.

This repo has no branches — work directly on `main` and commit there. Don't create feature branches or PRs.

## Stack

Tauri 2.x shell · Rust core (`src-tauri/`) · React + TypeScript frontend (`src/`) · xterm.js (DOM renderer) for terminal rendering · `portable-pty` for PTY spawning · JSON file persistence (no SQLite) · `tethys-hook` companion binary (`crates/tethys-hook/`) that forwards Claude Code hooks over a Unix socket.

## Running

```
pnpm tauri dev
```

State lives at `~/Library/Application Support/app.tethys.dev/` (`state.json`, `logs/`, `repos.toml`, auto-generated `repos.schema.json`, `hook.sock`).

Tethys writes its hook entries into `~/.claude/settings.json` on every boot (keyed by `description: "Tethys session monitor"`). They're idempotent — safe to leave across reinstalls.

It also generates a `CLAUDE.md` at each workspace root (`workspace_doc.rs`) explaining the worktree layout and telling sessions to ask for a missing repo rather than reading some other checkout. Rewritten on create, on repo-add, and at every boot. Claude Code reads CLAUDE.md from every parent dir, so the root file also applies to per-repo sessions.

The prose lives in `repos.toml`, not in Rust: `[workspace_doc].body` (with `{branch}` / `{repo_list}` / `{available_repos}` / `{workspace_root}` / `{clone_dir}` placeholders) falling back to `DEFAULT_BODY`, plus per-repo `claude_notes`. Rust owns only the marker line and the "Repo notes" section.

## Rust

Use idiomatic rust. After a set of changes are finished, run clippy and clean up the issues it reports

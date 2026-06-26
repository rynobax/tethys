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

## Rust

Use idiomatic rust. After a set of changes are finished, run clippy and clean up the issues it reports

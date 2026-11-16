# Tethys

A macOS desktop app for running multiple Claude Code CLI sessions in parallel across git worktrees.

Each "workspace" bundles N git worktrees (one per repo) with the Claude sessions running inside them. A companion hook binary listens to Claude Code's hook events so the app can flag "your turn" when a session is waiting on you, alongside live PR/CI status from GitHub.

## Stack

Electron (`electron/`) · Rust core (`src-tauri/`) run as a headless daemon, `tethysd` · React + TypeScript (`src/`) · xterm.js for terminal rendering · `portable-pty` for PTY spawning · JSON file persistence · `tethys-hook` companion binary (`crates/tethys-hook/`) that forwards Claude Code hooks over a Unix socket.

## Running

```
pnpm install
pnpm dev
```

State lives at `~/Library/Application Support/app.tethys.dev/`. On boot, Tethys writes idempotent hook entries into `~/.claude/settings.json` (keyed by `description: "Tethys session monitor"`).

## Status

Personal tool, macOS-only, not packaged for distribution

# Sessions

- The Initial prompt is pasted when the TUI enables bracketed paste (`tui_ready`), not on SessionStart: codex fires SessionStart at its first turn, not at startup.
- `last_active_at` is persisted because boot reattaches every tmux pane, which resets tmux's own activity time.

## Turns outlive sessions

A turn left unread is still Ryan's after the session closes, whether by the idle close, the agent exiting, or a restart that found its tmux gone. The child exiting leaves an unread turn as it was (the tracker remembers the exit, so nothing from the dead session can change it) rather than going `Dormant`, and with no live session this run the frontend reads the turn off the saved `runtime_state` and `turn_acknowledged`. Dismissing it works the same either way.

## Strays

A stray is a process whose `TETHYS_SESSION_ID` or `TETHYS_SPAWN_TOKEN` names no live tmux session. Detached processes keep both tags, and macOS lets us read the environment of a process we own. Either tag counts as live, because old sessions carry only the token and codex may filter `*TOKEN*` vars.

The safeguards in `strays.rs` are the point; don't loosen them:

- If tmux can't list every session, a missing one would read as dead, so the sweep is skipped.
- Anything sharing Tethys's own tags is spared, because `pnpm tauri dev` run inside a session carries that session's tag.
- tmux and `memwatch.sh` outlive Tethys and may carry a dead session's tag. Killing the tmux server kills every session.

## Memory budget

macOS enforces no memory limit (`ulimit -v` refused, `RLIMIT_RSS` ignored), so a runaway test swaps the machine to a halt. `budget.rs` is that limit.

- It measures footprint, not RSS, which leaves out compressed pages and understates exactly when memory is short.
- The agent is told why it was killed, because an unexplained exit 143 gets rerun as a flaky test. The notice must be recorded before the signal, so the killed command's own `PostToolUse(Failure)` hook can carry it back.
- The budget request's sidebar dot is OR'd in on the frontend rather than set as turn state, because the probe would flip `WaitingInput` back to `Working` within 2s.

Claude's probe reports a turn ending with a background shell as `shell` (Working). A dev server never finishes, so its workspace never went idle. The budget sweep therefore also marks agents whose background shells all hold listening sockets, and those read as Idle.

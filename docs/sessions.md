# Sessions

A workspace runs one agent session, at the workspace root whatever the repo count (`Workspace::session_cwd`). Sessions started before that rule keep their old cwd inside a worktree.

## Opening

Start, Resume and Reconnect are all `sessions::open_session`. It does the least that works: return the running handle, reattach a surviving tmux pane, resume a transcript that's actually on disk (`transcript_is_resumable`; an empty one fails with "No conversation found"), else start fresh. The tmux session name is the session id, so a new tmux session means new meta.

`switch_agent` kills the tmux session first so reattach can't win. Switching across agents drops the meta, since neither CLI can read the other's transcript.

The Initial prompt is pasted once the TUI turns bracketed paste on (`ESC[?2004h`, surfaced as `tui_ready`). Not SessionStart: codex fires that at its first turn, not at startup.

## Idle close

A session untouched for 24 hours has its tmux session killed (`idle.rs`); the transcript stays and Resume picks it up. Keystrokes and hooks count as touches. `last_active_at` is persisted because boot reattaches every pane, which resets tmux's own activity time. A `Working` session is never closed.

## Strays

`strays.rs` kills processes whose `TETHYS_SESSION_ID` or `TETHYS_SPAWN_TOKEN` names no live tmux session: dev servers and orphaned test runs that reparented to launchd. Its safeguards matter more than its rule:

- Skip the sweep if tmux can't list every session and its environment.
- Snapshot processes before listing sessions, so a new session can't be missed.
- Spare Tethys, its ancestors, its children and anything sharing its own tags (`pnpm tauri dev` run inside a session).
- Spare tmux and `memwatch.sh` and their subtrees by name; killing the tmux server takes every session.
- Re-check each pid's start time before each signal.

## Memory budget

macOS enforces no memory limit on a process tree, so a runaway test swaps the machine to a halt instead of failing. `budget.rs` sums `ri_phys_footprint` per session tag every 3s and kills the biggest job of a session over 8 GB (`TETHYS_MEM_BUDGET`: GB or `off`). A job is a subtree under the agent, killed whole; the agent itself is never counted or killed. One job per sweep.

The agent is told why, since an unexplained exit 143 gets rerun as a flaky test. The notice is recorded before the signal and returned as `additionalContext` on the killed command's `PostToolUse` or `PostToolUseFailure`. `tethys-hook` waits for a reply only on those and `UserPromptSubmit` (`reply_event_name`). Codex is killed but not told.

`request_memory_budget` waits up to 30 minutes for Ryan to approve. Its sidebar dot is OR'd in on the frontend, not set as turn state, because the probe would flip `WaitingInput` back to `Working` within 2s. Grants live in memory only.

## Servers vs scripts

Claude's probe reports a turn ending with a background shell as `shell`, which reads as Working. A dev server never finishes, so its workspace was never idle. The budget sweep also records agents whose every Bash-tool background shell holds a listening socket, and `probe::state_from_status` reads `shell` as Idle for those. Runs even with the budget `off`.

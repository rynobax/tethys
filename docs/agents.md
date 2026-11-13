# Agents

The agent kind is stored, never derived from the binary name.

## Codex

Everything is passed on the command line, because `-c` outranks every config file, so the user's `~/.codex/config.toml` stays theirs. The one exception is trust.

- `--dangerously-bypass-hook-trust`: hooks defined at spawn are untrusted and silently don't run.
- `--sandbox workspace-write --ask-for-approval on-request`: a worktree is an untrusted project to codex, and untrusted defaults to read-only.
- `--add-dir` per repo git dir: they live outside the workspace root. Fixed at spawn, so a repo added later needs a restart.
- Hook `command` must be a shell string. The argv-array form fails config loading, and the pane exits 0 with the error scrolled away.
- The spawn token goes to `tethys-hook` as an argument, because `shell_environment_policy` can filter env vars.
- Trust can't be granted on the command line: `-c projects."<dir>".trust_level` is accepted and ignored. `codex_trust.rs` writes it to `~/.codex/config.toml`, keyed by resolved path (`/tmp` ≠ `/private/tmp`), and purge removes it.
- The probe is Claude-only, and codex sessions must be filtered out of reconciliation. A switched workspace keeps its cwd, so a dead Claude probe would heal a Claude id onto the codex session.

## Scrollback replay

Replaying the scrollback into a fresh xterm causes two bugs, both fixed in `scrollback.rs`:

- Recorded startup queries (`ESC[6n`, `ESC[c`, …) get answered again and the stale replies land as typed text. Strip them from the replay only, since live queries are awaited.
- tmux enables mouse reporting only on attach or resize. Once the ring has rolled past that, scrolling dies, so the tracked DEC modes are re-asserted after the replay.

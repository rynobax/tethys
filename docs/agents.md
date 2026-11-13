# Agents

`Workspace::agent` (`Claude` or `Codex`) is stored beside `agent_binary` and never derived from the binary name, since it decides hooks, resume and MCP wiring. There's no agent trait: `agent_cmd::build` is one function per agent because the two command lines share almost nothing.

## The codex command line

Codex is configured entirely with flags and `-c`; Tethys writes nothing into `~/.codex/config.toml` except trust (below).

- `--dangerously-bypass-hook-trust`: hooks defined at spawn are untrusted and silently don't run. Trusting them in config would leave a dead entry per spawn.
- `--sandbox workspace-write --ask-for-approval on-request`: the equivalent of plain `claude`. Spelled out because a worktree is untrusted to codex, and untrusted defaults to read-only.
- `--add-dir <git dir>` per repo: worktree git dirs live outside the workspace root. Fixed at spawn, so a repo added later isn't writable until restart.
- `-c project_doc_fallback_filenames=["CLAUDE.md"]`: reuse the generated workspace doc.
- Hook `command` must be a shell string, not an argv array. The array form fails config loading and the pane exits 0 with the error scrolled away.
- The spawn token goes to `tethys-hook` as an argument, since an env var can be filtered by `shell_environment_policy`.

## Trust

Codex asks to trust every new directory, and `-c projects."<dir>".trust_level` is accepted but ignored. So `codex_trust.rs` writes the stanza into `~/.codex/config.toml` with `toml_edit`, under the hook install's `flock`, and purge removes it. Keys are resolved paths (`/tmp` ≠ `/private/tmp`), resolved through the deepest existing ancestor so purge computes the same key after the worktree is gone.

## Probe

`probe.rs` is Claude-only: it heals the session id after compaction rotates it. Codex sessions are filtered out of reconciliation, or a dead Claude probe in the same cwd would heal a Claude id onto them.

## Scrollback replay

`PtyProcess::attach` replays the scrollback ring into a fresh xterm, and that replay causes two problems:

- Recorded startup queries (`ESC[6n`, `ESC]11;?`, `ESC[c`) get answered again and the stale replies land in the program as typed text. `scrollback::strip_queries` removes them from the replayed copy only.
- tmux enables mouse reporting only on attach or resize, so once the ring has rolled past that, scrolling dies. `scrollback::TermModes` tracks the DEC modes that matter and `attach` re-asserts them after the replay.

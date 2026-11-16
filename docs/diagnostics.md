# Diagnostics

`RUST_LOG` caps both log sinks, so raising `TETHYS_LOG_STDERR` past it does nothing.

The daemon's stdout carries only the `ready` line. Everything else, its children's output included, goes to stderr.

`memwatch.sh` detaches to outlive Tethys, which is one of the suspects. Stop it with `pkill -f memwatch.sh`. It reads footprint, not `ps rss`, which understates 2-3x under pressure. Before blaming Tethys, check `claude_kids_mb` (what sessions spawned) and `dockervm_mb`.

Tethys is two columns. `tethys_mb` is `tethysd` and its children. `ui_mb` is the Electron shell (`Electron` in dev, `Tethys` packaged) and its renderer and GPU helpers: the shell is the daemon's parent, so it's found as that, never by name alone. A daemon started from a terminal has no `ui_mb`.

## Isolated runs

`TETHYS_DATA_DIR=<dir>` runs a second Tethys beside the real one. It uses its own tmux server (`tethys-iso-<hash>` of the dir) and skips everything boot does outside the data dir: the Claude hook install, codex trust, memwatch, and the stray and memory-budget sweeps, which act on every tagged process on the machine.

Whatever the data dir, the daemon won't start while something answers on its `hook.sock`.

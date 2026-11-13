# Diagnostics

## Logs

`logging.rs` has two sinks: the file `logs/tethys.log.<date>` at `info,tethys_lib=debug` (the real log), and stderr at `warn`, since it's an unbounded pipe into a terminal. `TETHYS_LOG_STDERR` overrides stderr. `RUST_LOG` caps both.

## Memory watchdog

Tethys launches `scripts/memwatch.sh` at boot (`memwatch.rs`; `TETHYS_MEMWATCH=off` disables it, an integer sets the interval). It samples every 20s into `~/memwatch/samples.tsv` and dumps `snap-<ts>.txt` on system pressure: a process over 8GB, swap over 12GB, or free under 250MB. It's a pidfile singleton that detaches to outlive Tethys, which is one of the suspects. Stop it with `pkill -f memwatch.sh`.

It measures `phys_footprint` (Activity Monitor's number) from `top`, not `ps rss`, which understates 2-3x under pressure. Check `claude_kids_mb` (what sessions spawned) and `dockervm_mb` before blaming Tethys; the machine is 24GB and routinely oversubscribed.

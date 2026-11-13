# Diagnostics

`RUST_LOG` caps both log sinks, so raising `TETHYS_LOG_STDERR` past it does nothing.

`memwatch.sh` detaches to outlive Tethys, which is one of the suspects. Stop it with `pkill -f memwatch.sh`. It reads footprint, not `ps rss`, which understates 2-3x under pressure. Before blaming Tethys, check `claude_kids_mb` (what sessions spawned) and `dockervm_mb`.

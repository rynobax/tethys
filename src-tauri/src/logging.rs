use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{
    fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer as _,
};

/// Verbosity of the stderr mirror when `TETHYS_LOG_STDERR` is unset. The file
/// layer is the real log; stderr is an unbounded pipe into whichever terminal
/// launched the app (`pnpm tauri dev`), so it only carries what warrants
/// interrupting you.
const DEFAULT_STDERR_FILTER: &str = "warn";

/// Initialize global tracing: stderr mirror plus a rolling daily file in
/// `logs_dir`. Returned guard must live for the duration of the app — dropping
/// it flushes.
///
/// `RUST_LOG` sets the overall verbosity (and so caps both layers);
/// `TETHYS_LOG_STDERR` independently filters the stderr mirror — set it to
/// `off` to silence the terminal, or to `info,tethys_lib=debug` to get the
/// full firehose back.
pub fn init(logs_dir: &Path) -> WorkerGuard {
    let file_appender = tracing_appender::rolling::daily(logs_dir, "tethys.log");
    let (file_writer, guard) = tracing_appender::non_blocking(file_appender);

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,tethys_lib=debug"));

    let stderr_filter = EnvFilter::try_from_env("TETHYS_LOG_STDERR")
        .unwrap_or_else(|_| EnvFilter::new(DEFAULT_STDERR_FILTER));

    tracing_subscriber::registry()
        .with(filter)
        .with(
            fmt::layer()
                .with_writer(std::io::stderr)
                .with_ansi(true)
                .with_filter(stderr_filter),
        )
        .with(
            fmt::layer()
                .with_writer(file_writer)
                .with_ansi(false)
                .with_target(true),
        )
        .init();

    guard
}

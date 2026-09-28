use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{
    fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer as _,
};

/// Low because stderr is an unbounded pipe into the launching terminal's
/// scrollback; the file is the real log.
const DEFAULT_STDERR_FILTER: &str = "warn";

/// Hold the returned guard for the app's lifetime; dropping it flushes the file.
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

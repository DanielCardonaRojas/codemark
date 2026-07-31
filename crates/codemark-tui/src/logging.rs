//! Non-blocking file-based logging with subsystem targets.
//!
//! Writes to `$TMPDIR/codemark-tui.log`. Control verbosity via `RUST_LOG`:
//!
//! ```sh
//! RUST_LOG=codemark=debug        # all subsystems at debug
//! RUST_LOG=codemark::git=trace   # only git at trace
//! RUST_LOG=codemark::db=debug,codemark::http=debug
//! ```

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;

/// Initialize the logging subsystem.
///
/// Returns a [`WorkerGuard`] that **must** be held for the process lifetime.
/// Dropping it flushes pending log writes.
///
/// If a global tracing subscriber is already installed — which happens when the
/// TUI runs in-process inside a host that configured tracing first (e.g.
/// `codemark tui` bundled into the CLI) — this leaves that subscriber in place
/// instead of panicking, and the returned guard flushes an unused appender.
pub fn init_logging() -> WorkerGuard {
    let log_dir = std::env::temp_dir();
    let file_appender = tracing_appender::rolling::daily(log_dir, "codemark-tui.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("codemark=info"));

    // `try_init` errors only when a global subscriber already exists; degrade
    // gracefully rather than aborting the dashboard.
    if let Err(err) = fmt::Subscriber::builder()
        .with_env_filter(filter)
        .with_writer(non_blocking)
        .with_target(true)
        .with_thread_ids(true)
        .with_ansi(false)
        .try_init()
    {
        eprintln!("codemark-tui: tracing already initialized, using existing subscriber: {err}");
    }

    guard
}

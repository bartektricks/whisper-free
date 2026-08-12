//! Local, structured logging (plan §18).
//!
//! Logs go to a rolling file in the app log directory and to stderr in debug
//! builds. There is no telemetry and no network sink.
//!
//! Deliberately never logged: microphone audio, transcription text, clipboard
//! contents, dictionary contents. Call sites log *shapes* — durations, sample
//! counts, character counts — never the content itself.

use std::path::Path;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Initialise logging. The returned guard must be kept alive for the process
/// lifetime; dropping it stops the background writer and loses buffered lines.
pub fn init(log_dir: &Path) -> Option<WorkerGuard> {
    if let Err(e) = std::fs::create_dir_all(log_dir) {
        eprintln!("could not create log directory {}: {e}", log_dir.display());
        return None;
    }

    let file_appender = tracing_appender::rolling::daily(log_dir, "local-dictation.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let filter = EnvFilter::try_from_env("LOCAL_DICTATION_LOG")
        .unwrap_or_else(|_| EnvFilter::new("local_dictation_lib=info,warn"));

    let file_layer = fmt::layer()
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_target(true);

    let registry = tracing_subscriber::registry().with(filter).with(file_layer);

    #[cfg(debug_assertions)]
    let registry = registry.with(fmt::layer().with_writer(std::io::stderr));

    if registry.try_init().is_err() {
        // Already initialised (e.g. a second call in tests) — not fatal.
        return None;
    }

    Some(guard)
}

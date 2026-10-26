use std::fmt;
use std::path::Path;
use tracing::Level;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{
    fmt::{
        format::{FormatEvent, FormatFields, Writer},
        FmtContext,
    },
    layer::SubscriberExt,
    registry::LookupSpan,
    util::SubscriberInitExt,
    EnvFilter,
};

pub type Guard = WorkerGuard;

const MAX_LOG_BYTES: u64 = 10 * 1024 * 1024; // 10 MB

/// Terse stderr formatter: `[KNOT] LEVEL  message`
/// Stdout is owned exclusively by the MCP JSON-RPC transport.
pub struct KnotFormatter;

impl<S, N> FormatEvent<S, N> for KnotFormatter
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &tracing::Event<'_>,
    ) -> fmt::Result {
        write!(writer, "[KNOT] {}  ", level_tag(*event.metadata().level()))?;
        ctx.format_fields(writer.by_ref(), event)?;
        writeln!(writer)
    }
}

/// File formatter: `[YYYY-MM-DD HH:MM:SS] [LEVEL] message`
pub struct ActivityLogFormatter;

impl<S, N> FormatEvent<S, N> for ActivityLogFormatter
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &tracing::Event<'_>,
    ) -> fmt::Result {
        write!(
            writer,
            "[{}] [{}] ",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
            level_tag(*event.metadata().level()),
        )?;
        ctx.format_fields(writer.by_ref(), event)?;
        writeln!(writer)
    }
}

fn level_tag(level: Level) -> &'static str {
    match level {
        Level::ERROR => "ERROR",
        Level::WARN => "WARN ",
        Level::INFO => "INFO ",
        Level::DEBUG => "DEBUG",
        Level::TRACE => "TRACE",
    }
}

/// If the log file exceeds 10 MB, rotate it to `<name>.1` before the new
/// appender opens the file. Only one rotated copy is kept.
fn rotate_if_oversized(path: &Path) {
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() > MAX_LOG_BYTES {
            let mut rotated_name = path.file_name().unwrap_or_default().to_owned();
            rotated_name.push(".1");
            let rotated = path.with_file_name(rotated_name);
            let _ = std::fs::remove_file(&rotated);
            let _ = std::fs::rename(path, &rotated);
        }
    }
}

/// Initialise the global tracing subscriber. Must be called exactly once.
///
/// Returns a `Guard` that must be held for the duration of the process.
/// Dropping it flushes the non-blocking file writer's internal buffer.
pub fn init(filter: &str, log_path: Option<&Path>) -> Option<Guard> {
    let stderr = tracing_subscriber::fmt::layer()
        .event_format(KnotFormatter)
        .with_writer(std::io::stderr);

    match log_path {
        Some(path) => {
            rotate_if_oversized(path);
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let dir = path.parent().unwrap_or(Path::new("."));
            let file_name = path.file_name().unwrap_or_default();
            let appender = tracing_appender::rolling::never(dir, file_name);
            let (non_blocking, guard) = tracing_appender::non_blocking(appender);

            tracing_subscriber::registry()
                .with(EnvFilter::new(filter))
                .with(stderr)
                .with(
                    tracing_subscriber::fmt::layer()
                        .event_format(ActivityLogFormatter)
                        .with_writer(non_blocking)
                        .with_ansi(false),
                )
                .init();

            Some(guard)
        }
        None => {
            tracing_subscriber::registry()
                .with(EnvFilter::new(filter))
                .with(stderr)
                .init();
            None
        }
    }
}

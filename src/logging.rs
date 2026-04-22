use std::fmt;
use std::fs::OpenOptions;
use std::path::Path;
use std::sync::Mutex;
use tracing::Level;
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

fn open_log(path: &Path) -> Option<std::fs::File> {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    OpenOptions::new().create(true).append(true).open(path).ok()
}

/// Initialise the global tracing subscriber. Must be called exactly once.
/// When `log_path` is Some, events are also written to that file in
/// `[YYYY-MM-DD HH:MM:SS] [LEVEL] message` format.
pub fn init(filter: &str, log_path: Option<&Path>) {
    let stderr = tracing_subscriber::fmt::layer()
        .event_format(KnotFormatter)
        .with_writer(std::io::stderr);

    match log_path.and_then(open_log) {
        Some(file) => {
            tracing_subscriber::registry()
                .with(EnvFilter::new(filter))
                .with(stderr)
                .with(
                    tracing_subscriber::fmt::layer()
                        .event_format(ActivityLogFormatter)
                        .with_writer(Mutex::new(file))
                        .with_ansi(false),
                )
                .init();
        }
        None => {
            tracing_subscriber::registry()
                .with(EnvFilter::new(filter))
                .with(stderr)
                .init();
        }
    }
}

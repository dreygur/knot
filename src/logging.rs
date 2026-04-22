use std::fmt;
use tracing::Level;
use tracing_subscriber::{
    fmt::{
        format::{FormatEvent, FormatFields, Writer},
        FmtContext,
    },
    registry::LookupSpan,
};

/// Minimal log formatter: `[KNOT] LEVEL  message\n`
///
/// All output goes to stderr. Stdout is owned exclusively by the MCP JSON-RPC
/// transport and must never receive a byte from the application layer.
///
/// Format is intentionally terse - no timestamp, no module path, no thread id.
/// The `[KNOT]` prefix makes it trivial to grep or filter in shell pipelines.
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
        let level = *event.metadata().level();
        let level_str = level_tag(level);
        write!(writer, "[KNOT] {level_str}  ")?;
        ctx.format_fields(writer.by_ref(), event)?;
        writeln!(writer)
    }
}

/// Fixed-width, uppercase level tag - consistent column alignment.
fn level_tag(level: Level) -> &'static str {
    match level {
        Level::ERROR => "ERROR",
        Level::WARN => "WARN ",
        Level::INFO => "INFO ",
        Level::DEBUG => "DEBUG",
        Level::TRACE => "TRACE",
    }
}

/// Initialise the global tracing subscriber.
/// Must be called exactly once, before any `tracing::` macros fire.
pub fn init(filter: &str) {
    tracing_subscriber::fmt()
        .event_format(KnotFormatter)
        .with_writer(std::io::stderr)
        .with_env_filter(filter)
        .init();
}

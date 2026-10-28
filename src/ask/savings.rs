//! Estimated tokens Knot saved: what reading the files an answer points into would
//! have cost, minus the answer itself. Each code answer is logged as an event; the
//! status line sums them per Claude Code session and per response.
//!
//! Tokens are estimated as bytes / 4, the same rough rule Trail uses, so the two
//! tools' numbers are comparable but neither is exact.

use crate::ask::events::{self, read_tail};
use std::collections::HashSet;
use std::io::Read;
use std::path::Path;

/// The last prompt is near the end of the transcript; the first line holds the start.
const TRANSCRIPT_TAIL_BYTES: u64 = 512 * 1024;
const TRANSCRIPT_HEAD_BYTES: u64 = 64 * 1024;
/// The furthest back a status line refresh searches for the last prompt.
const TRANSCRIPT_MAX_BYTES: u64 = 64 * 1024 * 1024;

pub fn estimate_tokens(bytes: u64) -> usize {
    bytes.div_ceil(4) as usize
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Tokens saved by answering with `output` instead of reading `files` (repo-relative)
/// whole. Zero when the answer is as long as the files.
pub fn saved_tokens(root: &Path, files: &[String], output: &str) -> usize {
    let unique: HashSet<&String> = files.iter().collect();
    let file_bytes: u64 = unique
        .iter()
        .filter_map(|f| std::fs::metadata(root.join(f.as_str())).ok())
        .map(|m| m.len())
        .sum();
    estimate_tokens(file_bytes).saturating_sub(estimate_tokens(output.len() as u64))
}

/// Log what one answer saved as an event in the current session. Failures are ignored:
/// savings are a report, never a reason for a command to fail.
pub fn record(root: &Path, tool: &str, files: &[String], output: &str) {
    let saved = saved_tokens(root, files, output);
    if saved == 0 {
        return;
    }
    let bytes_before = files
        .iter()
        .collect::<HashSet<_>>()
        .iter()
        .filter_map(|f| std::fs::metadata(root.join(f.as_str())).ok())
        .map(|m| m.len())
        .sum();
    events::append(
        root,
        &events::Event {
            ts_ms: now_ms(),
            session: events::current_session(root),
            kind: events::EventKind::Answer,
            tool: tool.to_string(),
            bytes_before,
            bytes_after: output.len() as u64,
            saved_tokens: saved,
            learning: None,
            cost_minutes: None,
            cost_tokens: None,
        },
    );
}

fn read_head(path: &Path, max: u64) -> Option<String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path).ok()?.take(max).read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn timestamp_ms(line: &serde_json::Value) -> Option<i64> {
    let ts = line.get("timestamp")?.as_str()?;
    Some(chrono::DateTime::parse_from_rfc3339(ts).ok()?.timestamp_millis())
}

/// A prompt the person sent, not a tool result or injected context. Prompts with an
/// image or pasted text are stored as a list of parts rather than a string.
fn is_human_prompt(line: &serde_json::Value) -> bool {
    if line.get("type").and_then(|t| t.as_str()) != Some("user")
        || line.get("isMeta").and_then(|m| m.as_bool()) == Some(true)
    {
        return false;
    }
    match line.pointer("/message/content") {
        Some(serde_json::Value::String(_)) => true,
        Some(serde_json::Value::Array(parts)) => {
            !parts.is_empty() && parts.iter().all(|p| p.get("type").and_then(|t| t.as_str()) != Some("tool_result"))
        }
        _ => false,
    }
}

/// The last human prompt's time, searching ever larger tails: one turn of tool output
/// can be megabytes, so the prompt is not always near the end.
fn last_prompt_ms(transcript: &Path) -> Option<i64> {
    let len = std::fs::metadata(transcript).ok()?.len();
    let mut window = TRANSCRIPT_TAIL_BYTES;
    loop {
        let tail = read_tail(transcript, window)?;
        let found = tail
            .lines()
            .rev()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .find(is_human_prompt)
            .and_then(|v| timestamp_ms(&v));
        if found.is_some() || window >= len || window >= TRANSCRIPT_MAX_BYTES {
            return found;
        }
        // Clamped before the next read, so no read exceeds the cap
        window = (window * 8).min(TRANSCRIPT_MAX_BYTES);
    }
}

/// When the Claude Code session in `transcript` started, and when its last human
/// prompt was sent, both in milliseconds.
pub fn transcript_window(transcript: &Path) -> Option<(i64, i64)> {
    let head = read_head(transcript, TRANSCRIPT_HEAD_BYTES)?;
    let start = head
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .find_map(|v| timestamp_ms(&v))?;
    let last_prompt = last_prompt_ms(transcript).unwrap_or(start);
    Some((start, last_prompt))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_tokens_counts_each_file_once() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("big.rs"), "x".repeat(4_000)).unwrap();

        assert_eq!(saved_tokens(dir.path(), &["big.rs".into(), "big.rs".into()], &"y".repeat(400)), 900);
    }

    #[test]
    fn record_writes_an_answer_event() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("big.rs"), "x".repeat(4_000)).unwrap(); // 1000 tokens

        record(root, "ask", &["big.rs".into()], &"y".repeat(400));
        // An answer longer than the files saves nothing and is not written
        record(root, "map", &["big.rs".into()], &"y".repeat(8_000));

        let events = events::recent(root);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, events::EventKind::Answer);
        assert_eq!(events[0].tool, "ask");
        assert_eq!(events[0].bytes_after, 400);
        assert_eq!(events[0].saved_tokens, 900);
    }

    #[test]
    fn transcript_window_finds_start_and_last_human_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        let lines = [
            r#"{"type":"summary","timestamp":"2026-10-27T10:00:00.000Z"}"#,
            r#"{"type":"user","message":{"role":"user","content":"first"},"timestamp":"2026-10-27T10:00:01.000Z"}"#,
            r#"{"type":"user","message":{"role":"user","content":"second"},"timestamp":"2026-10-27T10:05:00.000Z"}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"look"},{"type":"image"}]},"timestamp":"2026-10-27T10:06:00.000Z"}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result"}]},"timestamp":"2026-10-27T10:07:00.000Z"}"#,
            r#"{"type":"user","isMeta":true,"message":{"role":"user","content":"hook context"},"timestamp":"2026-10-27T10:08:00.000Z"}"#,
        ];
        // A turn of tool output larger than the first tail window follows the prompt
        let filler = format!(r#"{{"type":"assistant","message":{{"content":"{}"}}}}"#, "x".repeat(700 * 1024));
        std::fs::write(&path, format!("{}\n{filler}\n", lines.join("\n"))).unwrap();

        let (start, prompt) = transcript_window(&path).unwrap();
        let ms = |s: &str| chrono::DateTime::parse_from_rfc3339(s).unwrap().timestamp_millis();
        assert_eq!(start, ms("2026-10-27T10:00:00Z"));
        assert_eq!(prompt, ms("2026-10-27T10:06:00Z"), "a prompt with an image counts, found past a long turn");
    }
}

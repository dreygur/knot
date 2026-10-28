//! Estimated tokens Knot saved: what reading the files an answer points into would
//! have cost, minus the answer itself. Each code answer appends one line to a ledger;
//! the status line sums it per Claude Code session and per response.
//!
//! Tokens are estimated as bytes / 4, the same rough rule Trail uses, so the two
//! tools' numbers are comparable but neither is exact.

use crate::ask::events::read_tail;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const LEDGER_FILE: &str = "savings.jsonl";
/// Only the end of the ledger is read; a session's entries are recent.
const LEDGER_TAIL_BYTES: u64 = 2 * 1024 * 1024;
/// The last prompt is near the end of the transcript; the first line holds the start.
const TRANSCRIPT_TAIL_BYTES: u64 = 512 * 1024;
const TRANSCRIPT_HEAD_BYTES: u64 = 64 * 1024;
/// The furthest back a status line refresh searches for the last prompt.
const TRANSCRIPT_MAX_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
struct Entry {
    ts_ms: i64,
    root: String,
    tool: String,
    saved: usize,
}

pub fn estimate_tokens(bytes: u64) -> usize {
    bytes.div_ceil(4) as usize
}

fn ledger_path(root: &Path) -> PathBuf {
    super::state_dir(root).join(LEDGER_FILE)
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

/// Append what one answer saved to the ledger. Failures are ignored: savings are a
/// report, never a reason for a command to fail.
pub fn record(root: &Path, tool: &str, files: &[String], output: &str) {
    record_at(&ledger_path(root), root, tool, files, output, now_ms());
}

fn record_at(ledger: &Path, root: &Path, tool: &str, files: &[String], output: &str, ts_ms: i64) {
    let saved = saved_tokens(root, files, output);
    if saved == 0 {
        return;
    }
    let entry = Entry { ts_ms, root: root.to_string_lossy().into_owned(), tool: tool.to_string(), saved };
    let Ok(line) = serde_json::to_string(&entry) else { return };
    let (Some(dir), Some(name)) = (ledger.parent(), ledger.file_name()) else { return };
    let Ok(ledger) = super::state_file(dir, &name.to_string_lossy()) else { return };
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(ledger) {
        let _ = writeln!(file, "{line}");
    }
}

fn read_head(path: &Path, max: u64) -> Option<String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path).ok()?.take(max).read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// Tokens saved in `root` at or after each of `since` (milliseconds), in order.
pub fn saved_since(root: &Path, since: &[i64]) -> Vec<usize> {
    saved_since_in(&ledger_path(root), root, since)
}

fn saved_since_in(ledger: &Path, root: &Path, since: &[i64]) -> Vec<usize> {
    let mut totals = vec![0; since.len()];
    let root = root.to_string_lossy();
    let Some(text) = read_tail(ledger, LEDGER_TAIL_BYTES) else { return totals };
    for entry in text.lines().filter_map(|l| serde_json::from_str::<Entry>(l).ok()) {
        if entry.root != root {
            continue;
        }
        for (total, &from) in totals.iter_mut().zip(since) {
            if entry.ts_ms >= from {
                *total += entry.saved;
            }
        }
    }
    totals
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

/// `850`, `12k`, `3.4M`.
pub fn short(tokens: usize) -> String {
    match tokens {
        0..=999 => tokens.to_string(),
        1_000..=999_999 => format!("{}k", (tokens + 500) / 1_000),
        _ => format!("{:.1}M", tokens as f64 / 1_000_000.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_savings_per_repo_and_sums_by_window() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("big.rs"), "x".repeat(4_000)).unwrap(); // 1000 tokens
        let ledger = dir.path().join("savings.jsonl");

        assert_eq!(saved_tokens(&root, &["big.rs".into(), "big.rs".into()], &"y".repeat(400)), 900);
        record_at(&ledger, &root, "ask", &["big.rs".into()], &"y".repeat(400), 1_000);
        record_at(&ledger, &root, "grep", &["big.rs".into()], &"y".repeat(800), 2_000);
        record_at(&ledger, &dir.path().join("other"), "ask", &["big.rs".into()], "", 2_500);
        // An answer longer than the files saves nothing and is not written
        record_at(&ledger, &root, "map", &["big.rs".into()], &"y".repeat(8_000), 3_000);

        assert_eq!(saved_since_in(&ledger, &root, &[0, 1_500, 5_000]), vec![1_700, 800, 0]);
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

    #[test]
    fn short_numbers() {
        assert_eq!(short(850), "850");
        assert_eq!(short(12_400), "12k");
        assert_eq!(short(3_400_000), "3.4M");
    }
}

//! The Claude Code session working in a repository, as Knot's hooks see it: which
//! transcript it writes, what it has cost so far, and which learnings it was shown.
//! State lives under the data dir; nothing is written into the repo.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

/// A session older than this is not credited with a new learning's cost.
const SESSION_FRESH_MS: i64 = 12 * 60 * 60 * 1000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRef {
    pub session_id: String,
    pub transcript_path: String,
    pub seen_ms: i64,
}

fn sessions_dir() -> PathBuf {
    super::default_cache_root().join("sessions")
}

fn repo_key(root: &Path) -> String {
    let hex = blake3::hash(root.to_string_lossy().as_bytes()).to_hex();
    hex[..16].to_string()
}

/// Session ids come from hook input; keep them to a safe file name.
fn safe_id(session_id: &str) -> String {
    session_id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(64).collect()
}

/// Note that `session` is the one working in `root` now.
pub fn remember(root: &Path, session: &SessionRef) {
    remember_in(&sessions_dir(), root, session);
}

fn remember_in(dir: &Path, root: &Path, session: &SessionRef) {
    let _ = std::fs::create_dir_all(dir);
    if let Ok(json) = serde_json::to_vec(session) {
        let _ = std::fs::write(dir.join(format!("{}.json", repo_key(root))), json);
    }
}

/// The session last seen working in `root`, if it was seen recently.
pub fn current(root: &Path) -> Option<SessionRef> {
    current_in(&sessions_dir(), root, chrono::Utc::now().timestamp_millis())
}

fn current_in(dir: &Path, root: &Path, now_ms: i64) -> Option<SessionRef> {
    let bytes = std::fs::read(dir.join(format!("{}.json", repo_key(root)))).ok()?;
    let session: SessionRef = serde_json::from_slice(&bytes).ok()?;
    (now_ms - session.seen_ms < SESSION_FRESH_MS).then_some(session)
}

/// Minutes since the session began and tokens it has used, read from its transcript.
/// Tokens count fresh input, cache writes and output; cache reads are re-reads of
/// context already paid for, so they are left out.
pub fn transcript_cost(transcript: &Path, now_ms: i64) -> Option<(u64, u64)> {
    let file = std::fs::File::open(transcript).ok()?;
    let mut start = None;
    let mut tokens = 0u64;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if !line.contains("\"usage\"") && start.is_some() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
        if start.is_none() {
            start = v
                .get("timestamp")
                .and_then(|t| t.as_str())
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                .map(|t| t.timestamp_millis());
        }
        if let Some(usage) = v.pointer("/message/usage") {
            for key in ["input_tokens", "cache_creation_input_tokens", "output_tokens"] {
                tokens += usage.get(key).and_then(|n| n.as_u64()).unwrap_or(0);
            }
        }
    }
    let minutes = ((now_ms - start?).max(0) / 60_000) as u64;
    Some((minutes, tokens))
}

fn shown_path(dir: &Path, session_id: &str) -> PathBuf {
    dir.join(format!("shown-{}.txt", safe_id(session_id)))
}

/// Of `ids`, the ones `session_id` has not been shown yet; those are marked shown.
pub fn first_showing(session_id: &str, ids: &[String]) -> Vec<String> {
    first_showing_in(&sessions_dir(), session_id, ids)
}

fn first_showing_in(dir: &Path, session_id: &str, ids: &[String]) -> Vec<String> {
    let path = shown_path(dir, session_id);
    let seen: HashSet<String> = std::fs::read_to_string(&path)
        .map(|s| s.lines().map(str::to_string).collect())
        .unwrap_or_default();
    let fresh: Vec<String> = ids.iter().filter(|id| !seen.contains(*id)).cloned().collect();
    if !fresh.is_empty() {
        let _ = std::fs::create_dir_all(dir);
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            for id in &fresh {
                let _ = writeln!(f, "{id}");
            }
        }
    }
    fresh
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_the_session_per_repo_until_it_goes_stale() {
        let dir = tempfile::tempdir().unwrap();
        let root = Path::new("/repo/a");
        let s = SessionRef { session_id: "s1".into(), transcript_path: "/t.jsonl".into(), seen_ms: 1_000 };
        remember_in(dir.path(), root, &s);
        assert_eq!(current_in(dir.path(), root, 2_000).unwrap().session_id, "s1");
        assert!(current_in(dir.path(), Path::new("/repo/b"), 2_000).is_none(), "other repos have their own");
        assert!(current_in(dir.path(), root, 1_000 + SESSION_FRESH_MS).is_none(), "stale after 12 hours");
    }

    #[test]
    fn transcript_cost_counts_minutes_and_paid_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        std::fs::write(
            &path,
            [
                r#"{"type":"user","timestamp":"2026-10-27T10:00:00.000Z","message":{"content":"hi"}}"#,
                r#"{"type":"assistant","message":{"usage":{"input_tokens":100,"cache_creation_input_tokens":2000,"cache_read_input_tokens":90000,"output_tokens":300}}}"#,
                r#"{"type":"assistant","message":{"usage":{"input_tokens":10,"output_tokens":20}}}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        let start = chrono::DateTime::parse_from_rfc3339("2026-10-27T10:00:00Z").unwrap().timestamp_millis();
        assert_eq!(transcript_cost(&path, start + 25 * 60_000), Some((25, 2_430)));
    }

    #[test]
    fn each_learning_is_shown_once_per_session() {
        let dir = tempfile::tempdir().unwrap();
        let ids = vec!["a".to_string(), "b".to_string()];
        assert_eq!(first_showing_in(dir.path(), "s1", &ids), ids);
        assert!(first_showing_in(dir.path(), "s1", &ids).is_empty());
        assert_eq!(first_showing_in(dir.path(), "s2", &ids[..1]), vec!["a".to_string()]);
        assert_eq!(first_showing_in(dir.path(), "../evil", &ids).len(), 2, "ids are made file-safe");
    }
}

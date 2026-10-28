//! The Claude Code session working in a repository, as Knot's hooks see it: which
//! transcript it writes, what it has cost so far, and which learnings it was shown.
//! State lives in the repo's git-ignored `.knot/cache/`, never in a commit.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

/// A session older than this is not credited with a new learning's cost.
const SESSION_FRESH_MS: i64 = 12 * 60 * 60 * 1000;
const SESSION_FILE: &str = "session.json";
const LAST_EDIT_FILE: &str = "last_edit";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRef {
    pub session_id: String,
    pub transcript_path: String,
    pub seen_ms: i64,
}

/// Session ids come from hook input; keep them to a safe file name.
fn safe_id(session_id: &str) -> String {
    session_id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(64).collect()
}

/// Note that `session` is the one working in `root` now.
pub fn remember(root: &Path, session: &SessionRef) {
    remember_in(&super::state_dir(root), session);
}

fn remember_in(dir: &Path, session: &SessionRef) {
    let Ok(path) = super::state_file(dir, SESSION_FILE) else {
        return;
    };
    if let Ok(json) = serde_json::to_vec(session) {
        let _ = std::fs::write(path, json);
    }
}

/// Note `rel_file` as the file the agent last edited in `root`, for the status line.
pub fn note_edit(root: &Path, rel_file: &str) {
    if let Ok(path) = super::state_file(&super::state_dir(root), LAST_EDIT_FILE) {
        let _ = std::fs::write(path, rel_file);
    }
}

/// The file the agent last edited in `root`. It is read from the repo's own folder, so
/// callers print it through `flatten`.
pub fn last_edit(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(super::state_dir(root).join(LAST_EDIT_FILE)).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// Where Claude Code writes transcripts: `$CLAUDE_CONFIG_DIR/projects`, else `~/.claude/projects`.
fn transcripts_dir() -> Option<PathBuf> {
    let config = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".claude")))?;
    Some(config.join("projects"))
}

/// `session.json` sits in the repo, so a cloned repo can plant one. Only a session seen
/// in the past whose transcript is a file Claude Code wrote (under `transcripts`) is used.
fn trusted(session: &SessionRef, transcripts: &Path, now_ms: i64) -> bool {
    let Ok(root) = std::fs::canonicalize(transcripts) else {
        return false;
    };
    let in_place = std::fs::canonicalize(&session.transcript_path).is_ok_and(|p| p.starts_with(&root) && p.is_file());
    session.seen_ms <= now_ms && now_ms - session.seen_ms < SESSION_FRESH_MS && in_place
}

/// The session last seen working in `root`, if it was seen recently.
pub fn current(root: &Path) -> Option<SessionRef> {
    current_in(&super::state_dir(root), &transcripts_dir()?, chrono::Utc::now().timestamp_millis())
}

fn current_in(dir: &Path, transcripts: &Path, now_ms: i64) -> Option<SessionRef> {
    let bytes = std::fs::read(dir.join(SESSION_FILE)).ok()?;
    let session: SessionRef = serde_json::from_slice(&bytes).ok()?;
    trusted(&session, transcripts, now_ms).then_some(session)
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

/// Of `ids`, the ones `session_id` has not been shown in `root` yet; those are marked shown.
pub fn first_showing(root: &Path, session_id: &str, ids: &[String]) -> Vec<String> {
    first_showing_in(&super::state_dir(root), session_id, ids)
}

fn first_showing_in(dir: &Path, session_id: &str, ids: &[String]) -> Vec<String> {
    let path = shown_path(dir, session_id);
    let seen: HashSet<String> = std::fs::read_to_string(&path)
        .map(|s| s.lines().map(str::to_string).collect())
        .unwrap_or_default();
    let fresh: Vec<String> = ids.iter().filter(|id| !seen.contains(*id)).cloned().collect();
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if let (false, Ok(path)) = (fresh.is_empty(), super::state_file(dir, &name)) {
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
        let repo_a = tempfile::tempdir().unwrap();
        let repo_b = tempfile::tempdir().unwrap();
        let transcripts = tempfile::tempdir().unwrap();
        let transcript = transcripts.path().join("t.jsonl");
        std::fs::write(&transcript, "").unwrap();
        let s = SessionRef { session_id: "s1".into(), transcript_path: transcript.to_string_lossy().into(), seen_ms: 1_000 };
        remember(repo_a.path(), &s);

        let dir = crate::ask::state_dir(repo_a.path());
        assert_eq!(current_in(&dir, transcripts.path(), 2_000).unwrap().session_id, "s1");
        assert!(dir.join(".gitignore").is_file(), "session state is never committed");
        let other = crate::ask::state_dir(repo_b.path());
        assert!(current_in(&other, transcripts.path(), 2_000).is_none(), "other repos have their own");
        assert!(current_in(&dir, transcripts.path(), 1_000 + SESSION_FRESH_MS).is_none(), "stale after 12 hours");
    }

    #[test]
    fn a_planted_session_is_ignored() {
        let transcripts = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let outside = elsewhere.path().join("secret");
        std::fs::write(&outside, "").unwrap();
        let inside = transcripts.path().join("t.jsonl");
        std::fs::write(&inside, "").unwrap();
        let session = |path: &Path, seen_ms| SessionRef {
            session_id: "s".into(),
            transcript_path: path.to_string_lossy().into(),
            seen_ms,
        };

        assert!(trusted(&session(&inside, 1_000), transcripts.path(), 2_000));
        assert!(!trusted(&session(&outside, 1_000), transcripts.path(), 2_000), "transcript outside Claude's folder");
        let escape = transcripts.path().join("..").join(elsewhere.path().file_name().unwrap()).join("secret");
        assert!(!trusted(&session(&escape, 1_000), transcripts.path(), 2_000), "a .. path is resolved first");
        assert!(!trusted(&session(transcripts.path(), 1_000), transcripts.path(), 2_000), "a folder is not a transcript");
        assert!(!trusted(&session(&inside, 9_000), transcripts.path(), 2_000), "seen in the future");
    }

    #[cfg(unix)]
    #[test]
    fn never_writes_through_a_symlinked_cache() {
        let repo = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        std::fs::create_dir(repo.path().join(".knot")).unwrap();
        std::os::unix::fs::symlink(target.path(), repo.path().join(".knot/cache")).unwrap();
        let s = SessionRef { session_id: "s1".into(), transcript_path: "/t".into(), seen_ms: 1 };

        remember(repo.path(), &s);
        first_showing(repo.path(), "s1", &["a".to_string()]);

        assert_eq!(std::fs::read_dir(target.path()).unwrap().count(), 0, "nothing lands where the link points");
    }

    #[cfg(unix)]
    #[test]
    fn never_writes_through_a_symlinked_state_file() {
        let repo = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let victim = target.path().join("authorized_keys");
        std::fs::write(&victim, "keep me").unwrap();
        let dir = crate::ask::state_dir(repo.path());
        std::fs::create_dir_all(&dir).unwrap();
        std::os::unix::fs::symlink(&victim, dir.join(SESSION_FILE)).unwrap();

        remember(repo.path(), &SessionRef { session_id: "s1".into(), transcript_path: "/t".into(), seen_ms: 1 });

        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep me");
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

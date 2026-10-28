//! A per-session log of what Knot saved: one JSON line per answer or swapped command,
//! so the status line and later reports can total it by Claude Code session.

use serde::{Deserialize, Serialize};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

const EVENTS_FILE: &str = "events.jsonl";
/// Only the end of the log is read; a session's events are recent.
const EVENTS_TAIL_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Answer,
    Swap,
    EditBlocked,
    Override,
    Learning,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub ts_ms: i64,
    pub session: String,
    pub kind: EventKind,
    pub tool: String,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub bytes_before: u64,
    #[serde(default)]
    pub bytes_after: u64,
    #[serde(default)]
    pub saved_tokens: usize,
    /// For `Learning` events: the learning file's stem, and what it took to work out.
    #[serde(default)]
    pub learning: Option<String>,
    #[serde(default)]
    pub cost_minutes: Option<u64>,
    #[serde(default)]
    pub cost_tokens: Option<u64>,
}

/// Append one event. Failures are ignored: the log is a report, never a reason for a
/// command to fail.
pub fn append(root: &Path, event: &Event) {
    // Unit tests never touch the real home
    let home = if cfg!(test) { None } else { dirs::home_dir() };
    append_to(root, event, home.as_deref());
}

/// [`append`], listing the repo in `home`'s repo list when this write creates the log.
fn append_to(root: &Path, event: &Event, home: Option<&Path>) {
    let Ok(line) = serde_json::to_string(event) else { return };
    let Ok(path) = super::state_file(&super::state_dir(root), EVENTS_FILE) else { return };
    let created = !path.exists();
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(file, "{line}");
        if let (true, Some(home)) = (created, home) {
            crate::card::repos::register(home, root);
        }
    }
}

/// A learning's id in the log: its file stem.
pub fn learning_id(file: &Path) -> String {
    file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Log that a repo learning was shown to the agent in the current session, with what it
/// took to work out when that was recorded.
pub fn learning_shown(root: &Path, session: &str, id: &str, cost_minutes: Option<u64>, cost_tokens: Option<u64>) {
    append(
        root,
        &Event {
            ts_ms: chrono::Utc::now().timestamp_millis(),
            session: session.to_string(),
            kind: EventKind::Learning,
            tool: "learning".to_string(),
            command: None,
            bytes_before: 0,
            bytes_after: 0,
            saved_tokens: 0,
            learning: Some(id.to_string()),
            cost_minutes,
            cost_tokens,
        },
    );
}

/// Every event in the log, oldest first; lines that do not parse are skipped.
pub fn all(root: &Path) -> Vec<Event> {
    let path = super::state_dir(root).join(EVENTS_FILE);
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

/// The events in the last 2 MiB of the log, oldest first.
pub fn recent(root: &Path) -> Vec<Event> {
    let path = super::state_dir(root).join(EVENTS_FILE);
    let Some(text) = read_tail(&path, EVENTS_TAIL_BYTES) else { return Vec::new() };
    text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

/// Tokens `session` saved at or after each of `since` (milliseconds), in order.
pub fn saved_since(root: &Path, session: &str, since: &[i64]) -> Vec<usize> {
    let mut totals = vec![0; since.len()];
    for event in recent(root).iter().filter(|e| e.session == session) {
        for (total, &from) in totals.iter_mut().zip(since) {
            if event.ts_ms >= from {
                *total += event.saved_tokens;
            }
        }
    }
    totals
}

/// The Claude Code session id Knot last saw in this repo, or empty when there is none.
pub fn current_session(root: &Path) -> String {
    super::session::current(root).map(|s| s.session_id).unwrap_or_default()
}

pub(crate) fn read_tail(path: &Path, max: u64) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(max);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    // Drop a line cut in half by the seek
    Some(if start > 0 { text.split_once('\n').map_or(String::new(), |(_, rest)| rest.to_string()) } else { text })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(session: &str, ts_ms: i64, saved_tokens: usize) -> Event {
        Event {
            ts_ms,
            session: session.to_string(),
            kind: EventKind::Answer,
            tool: "knot_ask".to_string(),
            command: None,
            bytes_before: 0,
            bytes_after: 0,
            saved_tokens,
            learning: None,
            cost_minutes: None,
            cost_tokens: None,
        }
    }

    #[test]
    fn first_write_lists_the_repo_once() {
        let home = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        append_to(repo.path(), &event("a", 1, 5), Some(home.path()));
        let listed = crate::card::repos::list(home.path());
        assert_eq!(listed, vec![std::fs::canonicalize(repo.path()).unwrap()]);
        // Later writes do not register again, so a removed entry stays removed
        std::fs::remove_file(home.path().join(".knot/repos.txt")).unwrap();
        append_to(repo.path(), &event("a", 2, 5), Some(home.path()));
        assert!(crate::card::repos::list(home.path()).is_empty());
    }

    #[test]
    fn appends_and_reads_back_events() {
        let dir = tempfile::tempdir().unwrap();
        let mut swap = event("a", 2, 7);
        swap.kind = EventKind::Swap;
        swap.command = Some("git status".to_string());
        append(dir.path(), &event("a", 1, 5));
        append(dir.path(), &swap);

        let events = recent(dir.path());
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].saved_tokens, 5);
        assert_eq!(events[1].kind, EventKind::Swap);
        assert_eq!(events[1].command.as_deref(), Some("git status"));
    }

    #[test]
    fn totals_are_per_session() {
        let dir = tempfile::tempdir().unwrap();
        append(dir.path(), &event("a", 10, 100));
        append(dir.path(), &event("b", 10, 40));

        assert_eq!(saved_since(dir.path(), "a", &[0]), vec![100]);
        assert_eq!(saved_since(dir.path(), "b", &[0, 11]), vec![40, 0]);
    }

    #[test]
    fn learning_event_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        learning_shown(dir.path(), "s1", "2026-10-27-x", Some(12), Some(27_000));

        let events = all(dir.path());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, EventKind::Learning);
        assert_eq!(events[0].session, "s1");
        assert_eq!(events[0].learning.as_deref(), Some("2026-10-27-x"));
        assert_eq!(events[0].cost_minutes, Some(12));
        assert_eq!(events[0].cost_tokens, Some(27_000));
        assert_eq!(events[0].saved_tokens, 0);
    }

    #[test]
    fn old_lines_without_new_fields_still_parse() {
        let dir = tempfile::tempdir().unwrap();
        let line = r#"{"ts_ms":5,"session":"s","kind":"answer","tool":"knot_ask","bytes_before":40,"bytes_after":4,"saved_tokens":9}"#;
        let path = crate::ask::state_file(&crate::ask::state_dir(dir.path()), EVENTS_FILE).unwrap();
        std::fs::write(path, format!("{line}\nnot json\n")).unwrap();

        let events = all(dir.path());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].learning, None);
        assert_eq!(events[0].cost_tokens, None);
        assert_eq!(events[0].saved_tokens, 9);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_cache_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".knot")).unwrap();
        std::os::unix::fs::symlink(target.path(), dir.path().join(".knot/cache")).unwrap();

        append(dir.path(), &event("a", 1, 5));

        assert_eq!(std::fs::read_dir(target.path()).unwrap().count(), 0);
    }
}

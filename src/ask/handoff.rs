//! The note one session leaves for the next when it stops before the work is done:
//! the goal, what's done, the next step, open questions, files in progress. It lives in
//! the repo's git-ignored `.knot/cache/`, private to this machine.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

const HANDOFF_FILE: &str = "handoff.md";

/// A saved handoff and when it was written.
pub struct Handoff {
    pub body: String,
    pub saved: SystemTime,
}

/// Save `body` as the repo's handoff, replacing any earlier one.
pub fn save(root: &Path, body: &str) -> std::io::Result<PathBuf> {
    let path = super::state_file(&super::state_dir(root), HANDOFF_FILE)?;
    std::fs::write(&path, body)?;
    Ok(path)
}

/// Whether git tracks the note at `path`. A tracked one came with a clone, not from a
/// session on this machine, and could tell the agent anything.
fn tracked(root: &Path, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(root) else {
        return false;
    };
    git2::Repository::open(root)
        .and_then(|r| r.index())
        .is_ok_and(|index| index.get_path(rel, 0).is_some())
}

/// The handoff a session here saved in `root`, if one is waiting.
pub fn load(root: &Path) -> Option<Handoff> {
    let dir = super::state_dir(root);
    if !super::state_file_readable(&dir, HANDOFF_FILE) {
        return None;
    }
    let path = dir.join(HANDOFF_FILE);
    let body = std::fs::read_to_string(&path).ok()?;
    if body.trim().is_empty() || tracked(root, &path) {
        return None;
    }
    let saved = std::fs::metadata(&path).and_then(|m| m.modified()).ok()?;
    Some(Handoff { body, saved })
}

/// Remove the repo's handoff. `false` when there was none.
pub fn clear(root: &Path) -> std::io::Result<bool> {
    let dir = super::state_dir(root);
    if !dir.is_dir() {
        return Ok(false);
    }
    let path = super::state_file(&dir, HANDOFF_FILE)?;
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// The note's first heading or line, without its `#` marks.
pub fn title(body: &str) -> Option<&str> {
    body.lines().map(|l| l.trim_start_matches('#').trim()).find(|l| !l.is_empty())
}

/// "just now", "12 min ago", "2 h ago", "3 days ago".
pub fn age(saved: SystemTime, now: SystemTime) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    let secs = now.duration_since(saved).map_or(0, |d| d.as_secs());
    match secs {
        s if s < MINUTE => "just now".to_string(),
        s if s < HOUR => format!("{} min ago", s / MINUTE),
        s if s < DAY => format!("{} h ago", s / HOUR),
        s if s < 2 * DAY => "1 day ago".to_string(),
        s => format!("{} days ago", s / DAY),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const NOTE: &str = "# Session handoff for the retry change\n\n## Goal\nCap retries at four.\n";

    #[test]
    fn save_load_clear_round_trip() {
        let repo = tempfile::tempdir().unwrap();
        assert!(load(repo.path()).is_none());
        assert!(!clear(repo.path()).unwrap(), "nothing to clear yet");

        let path = save(repo.path(), NOTE).unwrap();
        assert_eq!(path, crate::ask::state_dir(repo.path()).join(HANDOFF_FILE));
        assert!(crate::ask::state_dir(repo.path()).join(".gitignore").is_file(), "the note is never committed");
        assert_eq!(load(repo.path()).unwrap().body, NOTE);

        assert!(clear(repo.path()).unwrap());
        assert!(load(repo.path()).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_cache_is_refused() {
        let repo = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        std::fs::create_dir(repo.path().join(".knot")).unwrap();
        std::os::unix::fs::symlink(target.path(), repo.path().join(".knot/cache")).unwrap();
        std::fs::write(target.path().join(HANDOFF_FILE), NOTE).unwrap();

        assert!(save(repo.path(), "overwrite").is_err());
        assert!(load(repo.path()).is_none(), "never read through the link");
        assert!(clear(repo.path()).is_err());
        assert_eq!(std::fs::read_to_string(target.path().join(HANDOFF_FILE)).unwrap(), NOTE);
    }

    #[test]
    fn a_note_committed_to_the_repo_is_ignored() {
        let repo = tempfile::tempdir().unwrap();
        let git = git2::Repository::init(repo.path()).unwrap();
        save(repo.path(), NOTE).unwrap();
        let mut index = git.index().unwrap();
        index.add_path(Path::new(".knot/cache/handoff.md")).unwrap();
        index.write().unwrap();

        assert!(load(repo.path()).is_none());
    }

    #[test]
    fn title_and_age_read_like_a_person_wrote_them() {
        assert_eq!(title(NOTE), Some("Session handoff for the retry change"));
        assert_eq!(title("\n\nfix the cache\nmore"), Some("fix the cache"));
        assert_eq!(title("  \n"), None);

        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10 * 24 * 3600);
        let ago = |secs| age(now - Duration::from_secs(secs), now);
        assert_eq!(ago(5), "just now");
        assert_eq!(ago(12 * 60), "12 min ago");
        assert_eq!(ago(2 * 3600 + 59), "2 h ago");
        assert_eq!(ago(30 * 3600), "1 day ago");
        assert_eq!(ago(3 * 24 * 3600), "3 days ago");
    }
}

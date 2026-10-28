//! The repos Knot has worked in, kept in `~/.knot/repos.txt` (one `.knot/cache` path per
//! line) so the card can total every repo's event log.

use anyhow::Result;
use std::io::Write;
use std::path::{Path, PathBuf};

const LIST_DIR: &str = ".knot";
const LIST_FILE: &str = "repos.txt";

/// Add `repo_root`'s state folder to the list in `home` if it is not there. Failures are
/// ignored: the list is a report, never a reason for a command to fail. Prints nothing.
pub fn register(home: &Path, repo_root: &Path) {
    let _ = try_register(home, repo_root);
}

/// The repo roots in the list (the parent of each `.knot/cache`), in the order they were
/// added, without repeats.
pub fn list(home: &Path) -> Vec<PathBuf> {
    let Ok(text) = std::fs::read_to_string(home.join(LIST_DIR).join(LIST_FILE)) else { return Vec::new() };
    let mut roots: Vec<PathBuf> = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let Some(root) = Path::new(line).parent().and_then(Path::parent) else { continue };
        if !roots.iter().any(|r| r == root) {
            roots.push(root.to_path_buf());
        }
    }
    roots
}

fn try_register(home: &Path, repo_root: &Path) -> Result<()> {
    let cache = std::fs::canonicalize(crate::ask::state_dir(repo_root))?;
    let entry = cache.to_string_lossy().into_owned();
    let dir = home.join(LIST_DIR);
    let path = dir.join(LIST_FILE);
    refuse_symlink(&dir)?;
    refuse_symlink(&path)?;
    let content = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => anyhow::bail!("cannot read {}: {e}", path.display()),
    };
    if content.lines().any(|l| l == entry) {
        return Ok(());
    }
    std::fs::create_dir_all(&dir)?;
    // One short O_APPEND write is atomic, so processes registering at once lose no line.
    // A race may list a repo twice; `list` dedupes.
    let line = if content.is_empty() || content.ends_with('\n') { format!("{entry}\n") } else { format!("\n{entry}\n") };
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
    file.write_all(line.as_bytes())?;
    Ok(())
}

fn refuse_symlink(path: &Path) -> Result<()> {
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        anyhow::bail!("{} is a symlink; Knot does not write through it", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_with_cache() -> tempfile::TempDir {
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join(".knot/cache")).unwrap();
        repo
    }

    #[test]
    fn register_twice_lists_once() {
        let home = tempfile::tempdir().unwrap();
        let repo = repo_with_cache();
        register(home.path(), repo.path());
        register(home.path(), repo.path());
        let root = std::fs::canonicalize(repo.path()).unwrap();
        assert_eq!(list(home.path()), vec![root]);
    }

    #[test]
    fn worktrees_sharing_a_cache_list_once() {
        let home = tempfile::tempdir().unwrap();
        let repo = repo_with_cache();
        let other = tempfile::tempdir().unwrap();
        let link = other.path().join("worktree");
        std::os::unix::fs::symlink(repo.path(), &link).unwrap();
        register(home.path(), repo.path());
        register(home.path(), &link);
        assert_eq!(list(home.path()).len(), 1);
    }

    #[test]
    fn symlinked_list_is_refused() {
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("target.txt");
        std::fs::write(&target, "keep\n").unwrap();
        std::fs::create_dir_all(home.path().join(".knot")).unwrap();
        std::os::unix::fs::symlink(&target, home.path().join(".knot/repos.txt")).unwrap();
        register(home.path(), repo_with_cache().path());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep\n");
    }

    #[test]
    fn concurrent_registrations_all_land() {
        let home = tempfile::tempdir().unwrap();
        let repos: Vec<_> = (0..16).map(|_| repo_with_cache()).collect();
        std::thread::scope(|s| {
            for repo in &repos {
                let home = home.path();
                s.spawn(move || register(home, repo.path()));
            }
        });
        assert_eq!(list(home.path()).len(), 16);
    }

    #[test]
    fn symlinked_knot_dir_is_refused() {
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), home.path().join(".knot")).unwrap();
        register(home.path(), repo_with_cache().path());
        assert!(!outside.path().join("repos.txt").exists());
    }
}

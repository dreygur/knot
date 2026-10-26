use anyhow::{Context, Result};
use git2::{BranchType, DiffOptions, Oid, Repository};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::JoinHandle;

const DEFAULT_POLL_INTERVAL_SECS: u64 = 180;
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
const REMOTE_HEAD_SUFFIX: &str = "/HEAD";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteCollision {
    pub file: String,
    pub branch: String,
    pub author: String,
    pub summary: String,
    pub commit_oid: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteCheckReport {
    pub collisions: Vec<RemoteCollision>,
    pub branches_inspected: usize,
    pub local_dirty_files: Vec<String>,
}

fn extract_diff_files(diff: &git2::Diff) -> HashSet<String> {
    let mut files = HashSet::new();
    let num_deltas = diff.deltas().len();
    for i in 0..num_deltas {
        if let Some(delta) = diff.get_delta(i) {
            let path_str = delta
                .new_file()
                .path()
                .unwrap_or_else(|| Path::new(""))
                .to_string_lossy()
                .replace('\\', "/");
            if !path_str.is_empty() {
                files.insert(path_str);
            }
        }
    }
    files
}

fn collect_local_dirty_files(repo: &Repository) -> Result<HashSet<String>> {
    let mut diff_opts = DiffOptions::new();
    diff_opts.include_untracked(true);

    let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
    let diff = repo.diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut diff_opts))?;
    Ok(extract_diff_files(&diff))
}

fn files_changed_in_remote_branch(
    repo: &Repository,
    base_oid: Oid,
    branch_oid: Oid,
) -> Result<HashSet<String>> {
    let base_commit = repo.find_commit(base_oid)?;
    let branch_commit = repo.find_commit(branch_oid)?;

    let base_tree = base_commit.tree()?;
    let branch_tree = branch_commit.tree()?;

    let diff = repo.diff_tree_to_tree(Some(&base_tree), Some(&branch_tree), None)?;
    Ok(extract_diff_files(&diff))
}

/// Detect collisions between uncommitted local edits and remote tracking branches.
pub fn detect_remote_collisions(repo_root: &Path) -> Result<RemoteCheckReport> {
    let repo = Repository::discover(repo_root).context("Failed to open git repository")?;
    let local_files = collect_local_dirty_files(&repo)?;

    let mut local_sorted: Vec<String> = local_files.iter().cloned().collect();
    local_sorted.sort();

    if local_files.is_empty() {
        return Ok(RemoteCheckReport {
            collisions: Vec::new(),
            branches_inspected: 0,
            local_dirty_files: Vec::new(),
        });
    }

    let head_oid = match repo.head().ok().and_then(|h| h.target()) {
        Some(oid) => oid,
        None => {
            return Ok(RemoteCheckReport {
                collisions: Vec::new(),
                branches_inspected: 0,
                local_dirty_files: local_sorted,
            });
        }
    };

    let branches = repo.branches(Some(BranchType::Remote))?;
    let mut collisions = Vec::new();
    let mut branches_inspected = 0;

    for branch_res in branches {
        let (branch, _) = branch_res?;
        let branch_name = branch.name()?.unwrap_or("").to_string();

        if branch_name.is_empty() || branch_name.ends_with(REMOTE_HEAD_SUFFIX) {
            continue;
        }

        let Some(branch_target) = branch.get().target() else {
            continue;
        };

        if branch_target == head_oid {
            continue;
        }

        let Ok(merge_base) = repo.merge_base(head_oid, branch_target) else {
            continue;
        };

        if merge_base == branch_target {
            continue;
        }

        branches_inspected += 1;
        let remote_files = files_changed_in_remote_branch(&repo, merge_base, branch_target)?;

        let commit = repo.find_commit(branch_target)?;
        let author = commit.author().name().unwrap_or("unknown").to_string();
        let summary = commit.summary().unwrap_or("").to_string();
        let commit_oid = branch_target.to_string();

        for file in local_files.intersection(&remote_files) {
            collisions.push(RemoteCollision {
                file: file.clone(),
                branch: branch_name.clone(),
                author: author.clone(),
                summary: summary.clone(),
                commit_oid: commit_oid.clone(),
            });
        }
    }

    collisions.sort_by(|a, b| a.file.cmp(&b.file).then(a.branch.cmp(&b.branch)));

    Ok(RemoteCheckReport {
        collisions,
        branches_inspected,
        local_dirty_files: local_sorted,
    })
}

pub struct BranchMonitorHandle {
    latest_collisions: Arc<Mutex<Vec<RemoteCollision>>>,
    shutdown_tx: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl BranchMonitorHandle {
    pub fn get_collisions(&self) -> Vec<RemoteCollision> {
        self.latest_collisions
            .lock()
            .map(|l| l.clone())
            .unwrap_or_default()
    }

    pub fn stop(self) {
        let _ = self.shutdown_tx.send(true);
        self.task.abort();
    }
}

/// Refresh remote-tracking refs so collision checks see teammates' latest pushes.
///
/// Uses the `git` CLI so the user's credential helpers and SSH config apply. Prompts are
/// disabled and the fetch is bounded by a timeout so a missing credential never hangs
/// the monitor. Set `KNOT_NO_FETCH=1` to only inspect refs already fetched.
async fn fetch_remotes(repo_root: &Path) -> Result<()> {
    if std::env::var_os("KNOT_NO_FETCH").is_some() {
        return Ok(());
    }

    let mut cmd = tokio::process::Command::new("git");
    cmd.args(["fetch", "--all", "--prune", "--no-tags", "--quiet"])
        .current_dir(repo_root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    let output = tokio::time::timeout(FETCH_TIMEOUT, cmd.output())
        .await
        .context("git fetch timed out")??;
    if !output.status.success() {
        anyhow::bail!("git fetch failed: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(())
}

/// Spawn a background task to periodically check for collisions with remote branches.
pub fn start_branch_monitor(
    repo_root: PathBuf,
    interval_secs: Option<u64>,
) -> BranchMonitorHandle {
    let interval = Duration::from_secs(interval_secs.unwrap_or(DEFAULT_POLL_INTERVAL_SECS));
    let latest_collisions = Arc::new(Mutex::new(Vec::new()));
    let shared_state = Arc::clone(&latest_collisions);
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

    let task = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    break;
                }
                _ = ticker.tick() => {
                    if let Err(e) = fetch_remotes(&repo_root).await {
                        tracing::debug!("Remote fetch skipped: {e}");
                    }
                    match detect_remote_collisions(&repo_root) {
                        Ok(report) => {
                            for col in &report.collisions {
                                tracing::warn!(
                                    "Remote collision detected on file '{}' with branch '{}' by '{}'",
                                    col.file, col.branch, col.author
                                );
                            }
                            if let Ok(mut lock) = shared_state.lock() {
                                *lock = report.collisions;
                            }
                        }
                        Err(e) => {
                            tracing::debug!("Remote branch scan error: {e}");
                        }
                    }
                }
            }
        }
    });

    BranchMonitorHandle {
        latest_collisions,
        shutdown_tx,
        task,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use git2::Signature;
    use std::fs;
    use tempfile::TempDir;

    fn create_commit(
        repo: &Repository,
        file_path: &str,
        content: &str,
        message: &str,
    ) -> Result<Oid> {
        let workdir = repo.workdir().context("Missing workdir")?;
        let full_path = workdir.join(file_path);
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&full_path, content)?;

        let mut index = repo.index()?;
        index.add_path(Path::new(file_path))?;
        index.write()?;
        let tree_id = index.write_tree()?;
        let tree = repo.find_tree(tree_id)?;

        let sig = Signature::now("Teammate", "teammate@example.com")?;
        let parents = match repo.head().ok().and_then(|h| h.target()) {
            Some(parent_oid) => vec![repo.find_commit(parent_oid)?],
            None => vec![],
        };
        let parent_refs: Vec<&git2::Commit> = parents.iter().collect();

        let oid = repo.commit(
            Some("HEAD"),
            &sig,
            &sig,
            message,
            &tree,
            &parent_refs,
        )?;
        Ok(oid)
    }

    #[test]
    fn test_detect_remote_collisions_finds_overlap() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let repo = Repository::init(temp_dir.path())?;

        let first_commit = create_commit(&repo, "src/lib.rs", "// initial", "initial commit")?;

        let remote_commit = create_commit(
            &repo,
            "src/shared.rs",
            "pub fn shared_api() {}",
            "remote feature commit",
        )?;

        repo.reference(
            "refs/remotes/origin/feature_shared",
            remote_commit,
            true,
            "create mock remote tracking branch",
        )?;

        let first_obj = repo.find_object(first_commit, None)?;
        repo.reset(&first_obj, git2::ResetType::Hard, None)?;

        let workdir = repo.workdir().unwrap();
        fs::write(workdir.join("src/shared.rs"), "pub fn my_local_edit() {}")?;

        let report = detect_remote_collisions(temp_dir.path())?;
        assert_eq!(report.collisions.len(), 1);
        assert_eq!(report.collisions[0].file, "src/shared.rs");
        assert_eq!(report.collisions[0].branch, "origin/feature_shared");
        assert_eq!(report.collisions[0].author, "Teammate");

        Ok(())
    }

    fn git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(["-c", "user.name=Teammate", "-c", "user.email=t@example.com"])
            .args(args)
            .current_dir(dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    #[tokio::test]
    async fn test_fetch_picks_up_teammate_push() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let origin = temp_dir.path().join("origin.git");
        let mine = temp_dir.path().join("mine");
        let theirs = temp_dir.path().join("theirs");

        let seed = Repository::init(temp_dir.path().join("seed"))?;
        create_commit(&seed, "src/shared.rs", "pub fn v1() {}", "initial")?;
        git(temp_dir.path(), &["clone", "--quiet", "--bare", "seed", "origin.git"]);
        git(temp_dir.path(), &["clone", "--quiet", origin.to_str().unwrap(), "mine"]);
        git(temp_dir.path(), &["clone", "--quiet", origin.to_str().unwrap(), "theirs"]);

        fs::write(theirs.join("src/shared.rs"), "pub fn v2() {}")?;
        git(&theirs, &["checkout", "--quiet", "-b", "feature"]);
        git(&theirs, &["commit", "--quiet", "-am", "teammate change"]);
        git(&theirs, &["push", "--quiet", "origin", "feature"]);

        fs::write(mine.join("src/shared.rs"), "pub fn my_edit() {}")?;
        assert!(detect_remote_collisions(&mine)?.collisions.is_empty(), "push not fetched yet");

        fetch_remotes(&mine).await?;
        let report = detect_remote_collisions(&mine)?;
        assert_eq!(report.collisions.len(), 1);
        assert_eq!(report.collisions[0].branch, "origin/feature");
        Ok(())
    }
}

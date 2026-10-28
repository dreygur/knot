use anyhow::{Context, Result};
use git2::{BranchType, DiffOptions, Oid, Repository};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const POLL_INTERVAL: Duration = Duration::from_secs(180);
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
        let summary = commit.summary().ok().flatten().unwrap_or("").to_string();
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

/// A symbol you and a teammate both changed in the same file since your branches split.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolClash {
    pub file: String,
    pub symbol: String,
    pub branch: String,
    pub author: String,
    pub commit_oid: String,
    /// For a constant, its value before and after on their branch and in your tree.
    pub theirs: Option<(String, String)>,
    pub ours: Option<(String, String)>,
}

fn blob_text(repo: &Repository, commit: Oid, rel_file: &str) -> Option<String> {
    let tree = repo.find_commit(commit).ok()?.tree().ok()?;
    let entry = tree.get_path(Path::new(rel_file)).ok()?;
    let blob = repo.find_blob(entry.id()).ok()?;
    Some(String::from_utf8_lossy(blob.content()).into_owned())
}

/// Symbols by name, with each one's content hash and, for constants, its value.
fn symbol_states(rel_file: &str, source: &str) -> std::collections::HashMap<String, (String, Option<String>)> {
    let Some(lang) = crate::ast::SupportedLang::from_path(Path::new(rel_file)) else {
        return Default::default();
    };
    crate::ast::extract_file_symbols(rel_file, source, lang)
        .unwrap_or_default()
        .into_iter()
        .map(|s| {
            let value = (s.kind == crate::ast::SymbolKind::Constant)
                .then(|| constant_value(&s.signature, &s.name))
                .flatten();
            (s.name, (s.hash, value))
        })
        .collect()
}

/// `4` from `defaultRetryMax = 4` or `const MAX: u32 = 4;`.
fn constant_value(signature: &str, name: &str) -> Option<String> {
    let after_name = &signature[signature.find(name)? + name.len()..];
    let value = after_name.split_once('=')?.1;
    // A trailing comment is not part of the value
    let value = ["//", "/*", " #"].iter().fold(value, |v, marker| v.split(marker).next().unwrap_or(v));
    let value = value.trim().trim_end_matches([';', ',']).trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// Branches with no commit for this long are left out of clash reports.
const STALE_BRANCH_SECS: i64 = 60 * 24 * 60 * 60;

/// Names whose content differs between `base` and `changed`, added or removed included.
fn changed_names(
    base: &std::collections::HashMap<String, (String, Option<String>)>,
    changed: &std::collections::HashMap<String, (String, Option<String>)>,
) -> HashSet<String> {
    base.keys()
        .chain(changed.keys())
        .filter(|name| base.get(*name).map(|s| &s.0) != changed.get(*name).map(|s| &s.0))
        .cloned()
        .collect()
}

/// Symbols in `rel_file` whose content differs between HEAD and the working tree.
pub fn changed_symbols(repo_root: &Path, rel_file: &str) -> Vec<String> {
    let Ok(repo) = Repository::discover(repo_root) else { return Vec::new() };
    let Some(workdir) = repo.workdir() else { return Vec::new() };
    let ours = std::fs::read_to_string(workdir.join(rel_file)).unwrap_or_default();
    let head = repo
        .head()
        .ok()
        .and_then(|h| h.target())
        .and_then(|oid| blob_text(&repo, oid, rel_file))
        .unwrap_or_default();
    let mut names: Vec<String> =
        changed_names(&symbol_states(rel_file, &head), &symbol_states(rel_file, &ours)).into_iter().collect();
    names.sort();
    names
}

/// Symbols in `rel_file` that the working tree and some remote branch both changed
/// since they split. Reads only local remote-tracking refs; fetching is the caller's job.
pub fn symbol_clashes(repo_root: &Path, rel_file: &str) -> Result<Vec<SymbolClash>> {
    let repo = Repository::discover(repo_root).context("Failed to open git repository")?;
    let workdir = repo.workdir().context("bare repository")?.to_path_buf();
    let Ok(ours_text) = std::fs::read_to_string(workdir.join(rel_file)) else {
        return Ok(Vec::new());
    };
    let Some(head_oid) = repo.head().ok().and_then(|h| h.target()) else {
        return Ok(Vec::new());
    };
    // What you are changing: the working tree against your HEAD. Measuring from the
    // merge base instead would count everything main gained since an old branch split.
    let head = symbol_states(rel_file, &blob_text(&repo, head_oid, rel_file).unwrap_or_default());
    let ours = symbol_states(rel_file, &ours_text);
    let ours_changed = changed_names(&head, &ours);
    if ours_changed.is_empty() {
        return Ok(Vec::new());
    }
    let now = chrono::Utc::now().timestamp();

    let mut clashes = Vec::new();
    for branch_res in repo.branches(Some(BranchType::Remote))? {
        let (branch, _) = branch_res?;
        let branch_name = branch.name()?.unwrap_or("").to_string();
        let Some(tip) = branch.get().target() else { continue };
        if branch_name.is_empty() || branch_name.ends_with(REMOTE_HEAD_SUFFIX) || tip == head_oid {
            continue;
        }
        let Ok(base_oid) = repo.merge_base(head_oid, tip) else { continue };
        if base_oid == tip {
            continue;
        }
        let Ok(tip_commit) = repo.find_commit(tip) else { continue };
        if now - tip_commit.time().seconds() > STALE_BRANCH_SECS {
            continue;
        }
        let base_text = blob_text(&repo, base_oid, rel_file).unwrap_or_default();
        let Some(theirs_text) = blob_text(&repo, tip, rel_file) else { continue };
        if theirs_text == base_text {
            continue;
        }

        let base = symbol_states(rel_file, &base_text);
        let theirs = symbol_states(rel_file, &theirs_text);
        let mut both: Vec<String> = ours_changed.intersection(&changed_names(&base, &theirs)).cloned().collect();
        both.sort();

        let author = tip_commit.author().name().unwrap_or("unknown").to_string();
        let value = |states: &std::collections::HashMap<String, (String, Option<String>)>, name: &str| {
            states.get(name).and_then(|s| s.1.clone())
        };
        for name in both {
            clashes.push(SymbolClash {
                file: rel_file.to_string(),
                symbol: name.clone(),
                branch: branch_name.clone(),
                author: author.clone(),
                commit_oid: tip.to_string(),
                theirs: value(&base, &name).zip(value(&theirs, &name)),
                ours: value(&head, &name).zip(value(&ours, &name)),
            });
        }
    }
    Ok(clashes)
}

pub struct BranchMonitorHandle {
    latest_collisions: Arc<Mutex<Vec<RemoteCollision>>>,
}

impl BranchMonitorHandle {
    pub fn get_collisions(&self) -> Vec<RemoteCollision> {
        self.latest_collisions
            .lock()
            .map(|l| l.clone())
            .unwrap_or_default()
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
pub fn start_branch_monitor(repo_root: PathBuf) -> BranchMonitorHandle {
    let latest_collisions = Arc::new(Mutex::new(Vec::new()));
    let shared_state = Arc::clone(&latest_collisions);

    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(POLL_INTERVAL);
        loop {
            ticker.tick().await;
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
    });

    BranchMonitorHandle { latest_collisions }
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

    #[test]
    fn symbol_clashes_name_the_shared_symbol_and_both_values() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let seed = Repository::init(temp_dir.path().join("seed"))?;
        let v1 = "package r\n\nconst (\n\tdefaultRetryMax = 4\n\tdefaultWait = 1\n)\n\nfunc Do() {}\n";
        create_commit(&seed, "client.go", v1, "initial")?;
        git(temp_dir.path(), &["clone", "--quiet", "--bare", "seed", "origin.git"]);
        let origin = temp_dir.path().join("origin.git");
        git(temp_dir.path(), &["clone", "--quiet", origin.to_str().unwrap(), "mine"]);
        git(temp_dir.path(), &["clone", "--quiet", origin.to_str().unwrap(), "theirs"]);
        let (mine, theirs) = (temp_dir.path().join("mine"), temp_dir.path().join("theirs"));

        // They raise the retry count and also rewrite Do on a branch
        fs::write(theirs.join("client.go"), v1.replace("= 4", "= 10").replace("func Do() {}", "func Do() { retry() }"))?;
        git(&theirs, &["checkout", "--quiet", "-b", "kumar/retry"]);
        git(&theirs, &["-c", "user.name=Kumar", "-c", "user.email=k@x", "commit", "--quiet", "-am", "raise"]);
        git(&theirs, &["push", "--quiet", "origin", "kumar/retry"]);
        git(&mine, &["fetch", "--quiet"]);

        // Nothing edited yet: nothing to clash with
        assert!(symbol_clashes(&mine, "client.go")?.is_empty());

        // You lower the same constant and leave Do alone
        fs::write(mine.join("client.go"), v1.replace("= 4", "= 3"))?;
        let clashes = symbol_clashes(&mine, "client.go")?;
        assert_eq!(clashes.len(), 1, "{clashes:?}");
        let c = &clashes[0];
        assert_eq!((c.symbol.as_str(), c.branch.as_str(), c.author.as_str()), ("defaultRetryMax", "origin/kumar/retry", "Kumar"));
        assert_eq!(c.theirs, Some(("4".into(), "10".into())));
        assert_eq!(c.ours, Some(("4".into(), "3".into())));
        Ok(())
    }
}

use anyhow::Result;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::index::{build_doc, LexicalIndex, SymbolDoc};
use crate::ast::extract::{extract_file, SupportedLang};
use crate::ast::resolve::{list_supported_files, ParsedFile};
use crate::ast::CodeGraph;

/// Bump when the cached shape or tokenization changes, so old caches are ignored.
const CACHE_VERSION: u32 = 9;
const CACHE_SUBDIR: &str = "cache";
const CACHE_FILE: &str = "index.json";
/// Written into the cache folder so a repo without a `.knot/` rule never commits it.
const CACHE_GITIGNORE: &str = "*\n";

/// What a file looked like when it was cached; any difference means reparse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct Stamp {
    mtime_ns: u128,
    size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedFile {
    stamp: Stamp,
    parsed: ParsedFile,
    docs: Vec<SymbolDoc>,
}

#[derive(Debug, Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    files: HashMap<String, CachedFile>,
}

/// A searchable view of one repository: its code graph and lexical index.
pub struct Corpus {
    pub graph: CodeGraph,
    pub index: LexicalIndex,
}

/// Counts from one load, so callers and tests can see what the cache saved.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct LoadStats {
    pub reused: usize,
    pub parsed: usize,
}

fn stamp_of(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime_ns = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_nanos();
    Some(Stamp { mtime_ns, size: meta.len() })
}

/// What the status line reads instead of the full cache: per file stamps and totals,
/// small enough to load on every refresh.
#[derive(Debug, Serialize, Deserialize)]
struct StatusFile {
    version: u32,
    indexed_at_ms: u128,
    symbols: usize,
    stamps: HashMap<String, Stamp>,
}

/// How the cached index compares with the working tree, found by stat alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexStatus {
    /// False until some load has written the cache for this repository.
    pub indexed: bool,
    pub files: usize,
    pub symbols: usize,
    /// Files added, removed or changed since the cache was written.
    pub stale: usize,
}

/// A background re-index claimed longer ago than this is assumed dead.
const REINDEX_CLAIM_TTL: Duration = Duration::from_secs(120);

/// The folder under `cache_root` holding the index and the rest of a repo's local state.
pub fn state_dir_in(cache_root: &Path) -> PathBuf {
    cache_root.join(CACHE_SUBDIR)
}

/// The cache root belongs to one repository (its `.knot/`), so the file needs no key.
fn cache_path(cache_root: &Path) -> PathBuf {
    state_dir_in(cache_root).join(CACHE_FILE)
}

fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

/// A cloned repo can commit `.knot/cache` (or a file in it) as a symlink to anywhere,
/// say `~/.ssh`; writing through it would let the repo overwrite files outside itself.
fn refuse_symlink(path: &Path) -> std::io::Result<()> {
    if is_symlink(path) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{} is a symlink; Knot does not write through it", path.display()),
        ));
    }
    Ok(())
}

/// Create `dir` with a `.gitignore` covering everything in it, so a repo without a
/// `.knot` rule of its own never commits Knot's local state. Refuses when `dir` or
/// its parent (`.knot`) is a symlink.
fn create_state_dir(dir: &Path) -> std::io::Result<()> {
    if let Some(parent) = dir.parent() {
        refuse_symlink(parent)?;
    }
    refuse_symlink(dir)?;
    std::fs::create_dir_all(dir)?;
    let ignore = state_file(dir, ".gitignore")?;
    if !ignore.exists() {
        std::fs::write(ignore, CACHE_GITIGNORE)?;
    }
    Ok(())
}

/// `dir/name`, ready to write: the folder is created as [`create_state_dir`] does, and
/// the file must not be a symlink. Every write into a repo's state goes through this.
pub fn state_file(dir: &Path, name: &str) -> std::io::Result<PathBuf> {
    if !dir.is_dir() || is_symlink(dir) {
        create_state_dir(dir)?;
    }
    let path = dir.join(name);
    refuse_symlink(&path)?;
    Ok(path)
}

/// [`state_file`] for a path already joined, as the cache's own files are.
fn writable(path: &Path) -> std::io::Result<PathBuf> {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return Ok(path.to_path_buf());
    };
    state_file(dir, &name.to_string_lossy())
}

fn read_cache(path: &Path) -> HashMap<String, CachedFile> {
    let Ok(bytes) = std::fs::read(path) else {
        return HashMap::new();
    };
    match serde_json::from_slice::<CacheFile>(&bytes) {
        Ok(cache) if cache.version == CACHE_VERSION => cache.files,
        Ok(_) => HashMap::new(),
        Err(e) => {
            tracing::warn!("Ignoring unreadable ask cache {}: {e}", path.display());
            HashMap::new()
        }
    }
}

/// Write via a temp file and rename so a crash never leaves a half written cache.
fn write_cache(path: &Path, files: HashMap<String, CachedFile>) -> Result<()> {
    let path = writable(path)?;
    let tmp = writable(&path.with_extension("json.tmp"))?;
    std::fs::write(&tmp, serde_json::to_vec(&CacheFile { version: CACHE_VERSION, files })?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn status_path(cache_file: &Path) -> PathBuf {
    cache_file.with_extension("status.json")
}

fn claim_path(cache_file: &Path) -> PathBuf {
    cache_file.with_extension("reindex")
}

fn write_status(cache_file: &Path, files: &HashMap<String, CachedFile>) -> Result<()> {
    let status = StatusFile {
        version: CACHE_VERSION,
        indexed_at_ms: SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0),
        // Distinct ids, as the graph counts them; a repeated id is one symbol there
        symbols: files
            .values()
            .flat_map(|f| f.parsed.symbols.iter().map(|s| s.id.as_str()))
            .collect::<std::collections::HashSet<_>>()
            .len(),
        stamps: files.iter().map(|(rel, f)| (rel.clone(), f.stamp)).collect(),
    };
    let path = writable(&status_path(cache_file))?;
    let tmp = writable(&path.with_extension("tmp"))?;
    std::fs::write(&tmp, serde_json::to_vec(&status)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Compare the working tree with the last written cache without parsing anything.
pub fn index_status(repo_root: &Path, cache_root: &Path) -> Result<IndexStatus> {
    let cache_file = cache_path(cache_root);
    let status = std::fs::read(status_path(&cache_file))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<StatusFile>(&bytes).ok())
        .filter(|s| s.version == CACHE_VERSION);
    let files = list_supported_files(repo_root)?;
    let Some(status) = status else {
        return Ok(IndexStatus { indexed: false, files: files.len(), symbols: 0, stale: files.len() });
    };

    let mut seen = 0;
    let mut stale = 0;
    for abs_path in &files {
        let rel_path = abs_path.strip_prefix(repo_root).unwrap_or(abs_path).to_string_lossy().replace('\\', "/");
        match status.stamps.get(&rel_path) {
            Some(stamp) => {
                seen += 1;
                if stamp_of(abs_path).as_ref() != Some(stamp) {
                    stale += 1;
                }
            }
            None => stale += 1,
        }
    }
    stale += status.stamps.len() - seen;
    Ok(IndexStatus { indexed: true, files: files.len(), symbols: status.symbols, stale })
}

/// Claim the right to start a background re-index of the repo owning `cache_root`, so
/// repeated status refreshes start one, not one each. A claim older than the TTL is taken over.
pub fn claim_reindex(cache_root: &Path) -> bool {
    let path = claim_path(&cache_path(cache_root));
    if let Ok(meta) = std::fs::metadata(&path) {
        let age = meta.modified().ok().and_then(|t| t.elapsed().ok());
        if age.is_some_and(|age| age < REINDEX_CLAIM_TTL) {
            return false;
        }
        let _ = std::fs::remove_file(&path);
    }
    if writable(&path).is_err() {
        return false;
    }
    std::fs::OpenOptions::new().write(true).create_new(true).open(&path).is_ok()
}

/// Drop a claim taken by [`claim_reindex`]; a missing claim is fine.
pub fn release_reindex(cache_root: &Path) {
    let _ = std::fs::remove_file(claim_path(&cache_path(cache_root)));
}

/// Parse one file and build its search docs from a single read.
fn parse_with_docs(repo_root: &Path, abs_path: &Path, stamp: Stamp) -> Option<(String, CachedFile)> {
    let rel_path = abs_path.strip_prefix(repo_root).unwrap_or(abs_path).to_string_lossy().replace('\\', "/");
    let lang = SupportedLang::from_path(abs_path)?;
    // A file that cannot be read as text or parsed is cached with no symbols, so it is
    // not retried on every load or reported as changed forever.
    let source = std::fs::read_to_string(abs_path).unwrap_or_default();
    let (symbols, imports) = extract_file(&rel_path, &source, lang).unwrap_or_default();

    let lines: Vec<&str> = source.lines().collect();
    let docs = symbols.iter().map(|s| build_doc(s, &lines)).collect();
    let parsed = ParsedFile { rel_path: rel_path.clone(), symbols, imports };
    Some((rel_path, CachedFile { stamp, parsed, docs }))
}

/// Load the corpus for `repo_root`, reparsing only files whose size or mtime
/// changed since the cache under `cache_root` was written. With no cache root
/// everything is parsed and nothing is written.
pub fn load_corpus(repo_root: &Path, cache_root: Option<&Path>) -> Result<(Corpus, LoadStats)> {
    let cache_file = cache_root.map(cache_path);
    let previous = cache_file.as_deref().map(read_cache).unwrap_or_default();

    let files = list_supported_files(repo_root)?;
    let loaded: Vec<(String, CachedFile, bool)> = files
        .par_iter()
        .filter_map(|abs_path| {
            let stamp = stamp_of(abs_path)?;
            let rel_path = abs_path.strip_prefix(repo_root).unwrap_or(abs_path).to_string_lossy().replace('\\', "/");
            match previous.get(&rel_path) {
                Some(cached) if cached.stamp == stamp => Some((rel_path, cached.clone(), true)),
                _ => parse_with_docs(repo_root, abs_path, stamp).map(|(rel, file)| (rel, file, false)),
            }
        })
        .collect();

    let stats = LoadStats {
        reused: loaded.iter().filter(|(_, _, reused)| *reused).count(),
        parsed: loaded.iter().filter(|(_, _, reused)| !*reused).count(),
    };
    let changed = stats.parsed > 0 || loaded.len() != previous.len();

    let mut parsed_files = Vec::with_capacity(loaded.len());
    let mut docs = Vec::new();
    let mut current = HashMap::with_capacity(loaded.len());
    for (rel_path, file, _) in loaded {
        parsed_files.push(file.parsed.clone());
        docs.extend(file.docs.iter().cloned());
        current.insert(rel_path, file);
    }

    if let Some(path) = cache_file {
        if changed || !status_path(&path).exists() {
            if let Err(e) = write_status(&path, &current) {
                tracing::warn!("Could not write index status {}: {e}", path.display());
            }
        }
        if changed {
            if let Err(e) = write_cache(&path, current) {
                tracing::warn!("Could not write ask cache {}: {e}", path.display());
            }
        }
    }

    docs.sort_by(|a, b| a.id.cmp(&b.id));
    let graph = CodeGraph::from_parsed_with_aliases(parsed_files, crate::ast::tsconfig::TsAliases::load(repo_root));
    let index = LexicalIndex::from_docs(docs);
    Ok((Corpus { graph, index }, stats))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_load_reuses_unchanged_files_and_reparses_edits() {
        let repo = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join("src")).unwrap();
        std::fs::write(repo.path().join("src/a.rs"), "pub fn alpha() {}\n").unwrap();
        std::fs::write(repo.path().join("src/b.rs"), "pub fn beta() {}\n").unwrap();

        let (_, first) = load_corpus(repo.path(), Some(cache.path())).unwrap();
        assert_eq!(first, LoadStats { reused: 0, parsed: 2 });

        let (_, second) = load_corpus(repo.path(), Some(cache.path())).unwrap();
        assert_eq!(second, LoadStats { reused: 2, parsed: 0 });

        std::fs::write(repo.path().join("src/b.rs"), "pub fn beta() {}\npub fn gamma_extra() {}\n").unwrap();
        let (corpus, third) = load_corpus(repo.path(), Some(cache.path())).unwrap();
        assert_eq!(third, LoadStats { reused: 1, parsed: 1 });
        assert!(corpus.graph.name_to_ids.contains_key("gamma_extra"));

        std::fs::remove_file(repo.path().join("src/a.rs")).unwrap();
        let (corpus, _) = load_corpus(repo.path(), Some(cache.path())).unwrap();
        assert!(!corpus.graph.name_to_ids.contains_key("alpha"), "deleted file must leave the corpus");
    }

    #[test]
    fn index_status_counts_changed_added_and_removed_files() {
        let repo = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        std::fs::write(repo.path().join("a.rs"), "pub fn alpha() {}\n").unwrap();
        std::fs::write(repo.path().join("b.rs"), "pub fn beta() {}\n").unwrap();

        let before = index_status(repo.path(), cache.path()).unwrap();
        assert!(!before.indexed);

        load_corpus(repo.path(), Some(cache.path())).unwrap();
        let fresh = index_status(repo.path(), cache.path()).unwrap();
        assert_eq!(fresh, IndexStatus { indexed: true, files: 2, symbols: 2, stale: 0 });

        std::fs::write(repo.path().join("a.rs"), "pub fn alpha() {}\npub fn more() {}\n").unwrap();
        std::fs::remove_file(repo.path().join("b.rs")).unwrap();
        std::fs::write(repo.path().join("c.rs"), "pub fn gamma() {}\n").unwrap();
        assert_eq!(index_status(repo.path(), cache.path()).unwrap().stale, 3);

        load_corpus(repo.path(), Some(cache.path())).unwrap();
        let reindexed = index_status(repo.path(), cache.path()).unwrap();
        assert_eq!((reindexed.stale, reindexed.symbols), (0, 3));
    }

    #[test]
    fn reindex_claim_is_exclusive_until_released() {
        let cache = tempfile::tempdir().unwrap();
        assert!(claim_reindex(cache.path()));
        assert!(!claim_reindex(cache.path()));
        release_reindex(cache.path());
        assert!(claim_reindex(cache.path()));
    }

    #[test]
    fn cache_lives_in_the_repo_and_is_git_ignored() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::write(repo.path().join("a.rs"), "pub fn alpha() {}\n").unwrap();
        let root = crate::ask::index_root(repo.path());

        load_corpus(repo.path(), Some(&root)).unwrap();

        let cache_dir = repo.path().join(".knot").join(CACHE_SUBDIR);
        assert!(cache_dir.join(CACHE_FILE).is_file());
        assert_eq!(std::fs::read_to_string(cache_dir.join(".gitignore")).unwrap(), CACHE_GITIGNORE);
        let (_, again) = load_corpus(repo.path(), Some(&root)).unwrap();
        assert_eq!(again, LoadStats { reused: 1, parsed: 0 }, "the cache folder itself is never indexed");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_knot_folder_is_never_written_through() {
        let repo = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        std::fs::write(repo.path().join("a.rs"), "pub fn alpha() {}\n").unwrap();
        std::os::unix::fs::symlink(target.path(), repo.path().join(".knot")).unwrap();

        let (corpus, _) = load_corpus(repo.path(), Some(&crate::ask::index_root(repo.path()))).unwrap();

        assert!(corpus.graph.name_to_ids.contains_key("alpha"), "the answer still works, uncached");
        assert!(!claim_reindex(&crate::ask::index_root(repo.path())));
        assert_eq!(std::fs::read_dir(target.path()).unwrap().count(), 0, "nothing lands where the link points");
    }

    #[test]
    fn stale_cache_version_is_ignored() {
        let repo = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        std::fs::write(repo.path().join("a.py"), "def alpha():\n    pass\n").unwrap();
        let path = cache_path(cache.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"version":0,"files":{}}"#).unwrap();

        let (_, stats) = load_corpus(repo.path(), Some(cache.path())).unwrap();
        assert_eq!(stats.parsed, 1);
    }
}

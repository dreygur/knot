use anyhow::Result;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use super::index::{build_doc, LexicalIndex, SymbolDoc};
use crate::ast::extract::{extract_file, SupportedLang};
use crate::ast::resolve::{list_supported_files, ParsedFile};
use crate::ast::CodeGraph;

/// Bump when the cached shape or tokenization changes, so old caches are ignored.
const CACHE_VERSION: u32 = 3;
const CACHE_SUBDIR: &str = "cache/ask";
const ROOT_KEY_HEX_CHARS: usize = 16;

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

/// One cache file per repository, named by a hash of its canonical path.
fn cache_path(cache_root: &Path, repo_root: &Path) -> PathBuf {
    let canonical = std::fs::canonicalize(repo_root).unwrap_or_else(|_| repo_root.to_path_buf());
    let key = blake3::hash(canonical.to_string_lossy().as_bytes()).to_hex();
    cache_root.join(CACHE_SUBDIR).join(format!("{}.json", &key[..ROOT_KEY_HEX_CHARS]))
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
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(&CacheFile { version: CACHE_VERSION, files })?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Parse one file and build its search docs from a single read.
fn parse_with_docs(repo_root: &Path, abs_path: &Path, stamp: Stamp) -> Option<(String, CachedFile)> {
    let rel_path = abs_path.strip_prefix(repo_root).unwrap_or(abs_path).to_string_lossy().replace('\\', "/");
    let lang = SupportedLang::from_path(abs_path)?;
    let source = std::fs::read_to_string(abs_path).ok()?;
    let (symbols, imports) = extract_file(&rel_path, &source, lang).ok()?;

    let lines: Vec<&str> = source.lines().collect();
    let docs = symbols.iter().map(|s| build_doc(s, &lines)).collect();
    let parsed = ParsedFile { rel_path: rel_path.clone(), symbols, imports };
    Some((rel_path, CachedFile { stamp, parsed, docs }))
}

/// Load the corpus for `repo_root`, reparsing only files whose size or mtime
/// changed since the cache under `cache_root` was written. With no cache root
/// everything is parsed and nothing is written.
pub fn load_corpus(repo_root: &Path, cache_root: Option<&Path>) -> Result<(Corpus, LoadStats)> {
    let cache_file = cache_root.map(|c| cache_path(c, repo_root));
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

    if let (Some(path), true) = (cache_file, changed) {
        if let Err(e) = write_cache(&path, current) {
            tracing::warn!("Could not write ask cache {}: {e}", path.display());
        }
    }

    docs.sort_by(|a, b| a.id.cmp(&b.id));
    let graph = CodeGraph::from_parsed(parsed_files);
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
    fn stale_cache_version_is_ignored() {
        let repo = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        std::fs::write(repo.path().join("a.py"), "def alpha():\n    pass\n").unwrap();
        let path = cache_path(cache.path(), repo.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"version":0,"files":{}}"#).unwrap();

        let (_, stats) = load_corpus(repo.path(), Some(cache.path())).unwrap();
        assert_eq!(stats.parsed, 1);
    }
}

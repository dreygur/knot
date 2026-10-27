mod cache;
pub mod grep;
mod index;
mod learnings;
pub mod map;
mod tokenize;

pub use cache::{claim_reindex, index_status, load_corpus, release_reindex, Corpus, IndexStatus, LoadStats};
pub use index::LexicalIndex;
pub use learnings::{flatten, related_learnings, LearningHit};
pub use tokenize::tokenize;

use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;

use crate::ast::{compute_weighted_pagerank, CodeGraph};

/// Lexical hits considered for the final ranking.
const LEXICAL_CANDIDATES: usize = 60;
/// Top lexical hits whose scores seed the graph walk.
const GRAPH_SEEDS: usize = 20;
/// Share of the final score taken from graph proximity to the lexical hits.
const GRAPH_WEIGHT: f64 = 0.25;
/// Test code rarely answers "how does X work"; keep it findable but below the real thing.
const TEST_PENALTY: f64 = 0.35;

/// One ranked answer to a question about the code.
#[derive(Debug, Clone, Serialize)]
pub struct AskHit {
    pub id: String,
    pub file: String,
    pub name: String,
    pub kind: String,
    pub signature: String,
    pub span: (usize, usize),
    pub crux: String,
    pub score: f64,
}

impl AskHit {
    /// `file:Lstart-Lend`, the pointer format both humans and the eval read.
    pub fn pointer(&self) -> String {
        format!("{}:L{}-L{}", self.file, self.span.0, self.span.1)
    }
}

/// Whether ranking blends in graph proximity or uses lexical scores alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ranking {
    Hybrid,
    LexicalOnly,
}

fn normalize_by_max(scores: &mut HashMap<String, f64>) {
    let max = scores.values().copied().fold(0.0f64, f64::max);
    if max > 0.0 {
        scores.values_mut().for_each(|s| *s /= max);
    }
}

/// Rank symbols for `query`: BM25F over names, signatures, docs, paths and bodies,
/// then a Personalized PageRank seeded by the strongest lexical hits so code that
/// sits between several matches rises.
pub fn rank(graph: &CodeGraph, index: &LexicalIndex, query: &str, limit: usize, ranking: Ranking) -> Vec<AskHit> {
    let lexical: Vec<(usize, f64)> = index.search(query).into_iter().take(LEXICAL_CANDIDATES).collect();
    if lexical.is_empty() {
        return Vec::new();
    }

    let mut lexical_scores: HashMap<String, f64> =
        lexical.iter().map(|(i, s)| (index.doc(*i).id.clone(), *s)).collect();
    normalize_by_max(&mut lexical_scores);

    let mut combined = lexical_scores.clone();
    if ranking == Ranking::Hybrid {
        let seeds: HashMap<String, f64> = lexical
            .iter()
            .take(GRAPH_SEEDS)
            .map(|(i, s)| (index.doc(*i).id.clone(), *s))
            .collect();
        let mut proximity: HashMap<String, f64> = compute_weighted_pagerank(graph, &seeds)
            .into_iter()
            .filter(|(id, _)| lexical_scores.contains_key(id))
            .collect();
        normalize_by_max(&mut proximity);

        for (id, score) in combined.iter_mut() {
            let graph_score = proximity.get(id).copied().unwrap_or(0.0);
            *score = (1.0 - GRAPH_WEIGHT) * *score + GRAPH_WEIGHT * graph_score;
        }
    }

    for (i, _) in &lexical {
        let doc = index.doc(*i);
        if doc.is_test {
            if let Some(score) = combined.get_mut(&doc.id) {
                *score *= TEST_PENALTY;
            }
        }
    }

    let mut ranked: Vec<(String, f64)> = combined.into_iter().collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));

    ranked
        .into_iter()
        .take(limit)
        .filter_map(|(id, score)| {
            let sym = graph.symbols.get(&id)?;
            Some(AskHit {
                id: sym.id.clone(),
                file: sym.file.clone(),
                name: sym.name.clone(),
                kind: sym.kind.as_str().to_string(),
                signature: sym.signature.clone(),
                span: sym.span,
                crux: sym.crux.clone(),
                score,
            })
        })
        .collect()
}

/// Where the ask cache lives: `$KNOT_DATA_DIR`, else `~/.knot`, matching the vault.
pub fn default_cache_root() -> std::path::PathBuf {
    std::env::var_os("KNOT_DATA_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".knot")))
        .unwrap_or_else(|| std::path::PathBuf::from(".knot"))
}

/// The repository `dir` belongs to: its git work tree, canonicalized, or `dir` itself
/// outside git. Code commands index this root so running them from a subdirectory sees
/// the whole repo, with the same paths and cache as running them from the top.
pub fn repo_root(dir: &Path) -> std::path::PathBuf {
    let root = git2::Repository::discover(dir)
        .ok()
        .and_then(|repo| repo.workdir().map(Path::to_path_buf))
        .unwrap_or_else(|| dir.to_path_buf());
    std::fs::canonicalize(&root).unwrap_or(root)
}

/// [`repo_root`] of the current directory.
pub fn current_repo_root() -> std::io::Result<std::path::PathBuf> {
    Ok(repo_root(&std::env::current_dir()?))
}

/// The code graph for `root`, reparsing only files changed since the last load, so
/// every caller sees the working tree as it is without a separate build step.
pub fn load_graph(root: &Path) -> anyhow::Result<crate::ast::CodeGraph> {
    let (corpus, _) = load_corpus(root, Some(&default_cache_root()))?;
    Ok(corpus.graph)
}

/// Load (or refresh) the corpus for `root` and answer one query. With a cache
/// root, only files changed since the last call are parsed.
pub fn ask(
    root: &Path,
    cache_root: Option<&Path>,
    query: &str,
    limit: usize,
    ranking: Ranking,
) -> anyhow::Result<Vec<AskHit>> {
    let (corpus, _) = load_corpus(root, cache_root)?;
    Ok(rank(&corpus.graph, &corpus.index, query, limit, ranking))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, file: &str, body: &str) {
        let path = root.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn ask_finds_documented_symbol_and_demotes_tests() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "src/http.rs",
            "/// Follow redirects until a final response arrives.\npub fn resolve_redirects() { send(); }\n\
             pub fn send() {}\n",
        );
        write(
            root,
            "tests/http_test.rs",
            "#[test]\nfn follows_redirects_test() { resolve_redirects(); }\n",
        );

        let hits = ask(root, None, "follow http redirects", 5, Ranking::Hybrid).unwrap();
        assert_eq!(hits[0].name, "resolve_redirects");
        assert_eq!(hits[0].pointer(), "src/http.rs:L2-L2");
        let test_rank = hits.iter().position(|h| h.name == "follows_redirects_test");
        assert!(test_rank.is_none_or(|r| r > 0));
    }

    #[test]
    fn empty_or_unmatched_query_returns_nothing() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "src/a.rs", "pub fn alpha() {}\n");
        assert!(ask(dir.path(), None, "zzzz qqqq", 5, Ranking::Hybrid).unwrap().is_empty());
        assert!(ask(dir.path(), None, "", 5, Ranking::LexicalOnly).unwrap().is_empty());
    }
}

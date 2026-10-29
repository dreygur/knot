mod cache;
pub mod events;
pub mod grep;
pub mod handoff;
mod index;
mod learnings;
pub mod map;
mod paths;
pub mod render;
pub mod savings;
pub mod session;
mod tokenize;
pub mod usage;

pub use cache::{
    claim_reindex, index_status, state_file, state_file_readable, load_corpus, release_reindex, IndexStatus,
};
pub use index::LexicalIndex;
pub use learnings::{flatten, mark_shown, related_learnings};
pub use tokenize::tokenize;

use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;

use crate::ast::{compute_weighted_pagerank, CodeGraph};
use crate::utils::count;

/// Lexical hits considered for the final ranking.
const LEXICAL_CANDIDATES: usize = 60;
/// Graph proximity to the lexical hits, added on top of the lexical score.
const GRAPH_WEIGHT: f64 = 0.5;
/// Boost per other candidate in the same file, on a log scale: a file holding many
/// matches is usually where the answer lives. Above this, held out sets got worse.
const FILE_WEIGHT: f64 = 0.05;
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

fn repo_files(graph: &CodeGraph) -> Vec<String> {
    let mut files: Vec<String> = graph.symbols.values().map(|s| s.file.clone()).collect();
    files.sort();
    files.dedup();
    files
}

/// One symbol for each counterpart of a file the query names: the best scored symbol
/// already in that file, else its first definition.
fn counterpart_symbols(graph: &CodeGraph, scored: &HashMap<String, f64>, named: &[&str], files: &[String]) -> Vec<String> {
    let mut picked = Vec::new();
    for named in named {
        for file in paths::counterparts(named, files) {
            let score = |id: &str| scored.get(id).copied().unwrap_or(0.0);
            let best = graph
                .symbols
                .values()
                .filter(|s| s.file == file)
                .max_by(|a, b| score(&a.id).total_cmp(&score(&b.id)).then(b.span.0.cmp(&a.span.0)));
            if let Some(sym) = best {
                picked.push(sym.id.clone());
            }
        }
    }
    picked
}

/// Rank symbols for `query`: BM25F over names, signatures, docs, paths and bodies,
/// then a Personalized PageRank seeded by every lexical hit so code that sits between
/// several matches rises, and a small boost for files holding many matches.
pub fn rank(graph: &CodeGraph, index: &LexicalIndex, query: &str, limit: usize, ranking: Ranking) -> Vec<AskHit> {
    let mut lexical: Vec<(usize, f64)> = index.search(query);
    if lexical.is_empty() {
        return Vec::new();
    }
    let tail = lexical.split_off(lexical.len().min(LEXICAL_CANDIDATES));

    let mut lexical_scores: HashMap<String, f64> =
        lexical.iter().map(|(i, s)| (index.doc(*i).id.clone(), *s)).collect();
    normalize_by_max(&mut lexical_scores);

    let mut combined = lexical_scores.clone();
    if ranking == Ranking::Hybrid {
        let seeds: HashMap<String, f64> = lexical.iter().map(|(i, s)| (index.doc(*i).id.clone(), *s)).collect();
        let mut proximity: HashMap<String, f64> = compute_weighted_pagerank(graph, &seeds)
            .into_iter()
            .filter(|(id, _)| lexical_scores.contains_key(id))
            .collect();
        normalize_by_max(&mut proximity);

        for (id, score) in combined.iter_mut() {
            *score += GRAPH_WEIGHT * proximity.get(id).copied().unwrap_or(0.0);
        }
    }

    let file_of = |id: &str| id.split("::").next().unwrap_or(id).to_string();
    let mut per_file: HashMap<String, usize> = HashMap::new();
    for id in lexical_scores.keys() {
        *per_file.entry(file_of(id)).or_default() += 1;
    }
    for (id, score) in combined.iter_mut() {
        let others = per_file.get(&file_of(id)).copied().unwrap_or(1).saturating_sub(1);
        *score += FILE_WEIGHT * (1.0 + others as f64).ln();
    }

    for (i, _) in &lexical {
        let doc = index.doc(*i);
        if doc.is_test {
            if let Some(score) = combined.get_mut(&doc.id) {
                *score *= TEST_PENALTY;
            }
        }
    }

    let files = repo_files(graph);
    let named = paths::named_files(query, &files);

    // The other side of a file the query names ranks with the best hit, test or not:
    // a failing test points at the code it tests, an edited file at its tests.
    let top = combined.values().copied().fold(0.0f64, f64::max);
    for id in counterpart_symbols(graph, &combined, &named, &files) {
        combined.insert(id, top);
    }

    let mut ranked: Vec<(String, f64)> = combined.into_iter().collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));

    // Matches past the reranked head keep their keyword order below it, so a caller
    // asking for many hits gets every matching file, not just the first few dozen.
    let floor = ranked.last().map_or(0.0, |(_, s)| *s);
    let tail_top = tail.first().map_or(1.0, |(_, s)| *s).max(f64::MIN_POSITIVE);
    let placed: std::collections::HashSet<String> = ranked.iter().map(|(id, _)| id.clone()).collect();
    ranked.extend(
        tail.iter()
            .map(|(i, s)| (index.doc(*i).id.clone(), floor * s / tail_top))
            .filter(|(id, _)| !placed.contains(id)),
    );

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

/// Where a repository's code index lives: its own `.knot/`, beside its learnings, so
/// the index sits with the code it describes the way `.trail/` does.
pub fn index_root(repo_root: &Path) -> std::path::PathBuf {
    repo_root.join(".knot")
}

/// A repository's local, never committed state: index, savings ledger, sessions and
/// the fetch marker, all in `.knot/cache/`. Only the log and the handoff records stay in `~/.knot`.
pub fn state_dir(repo_root: &Path) -> std::path::PathBuf {
    cache::state_dir_in(&index_root(repo_root))
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

/// The code index, learnings and python block of `root`, for status output.
pub fn repo_summary(root: &Path) -> String {
    let index = match index_status(root, &index_root(root)) {
        Err(e) => format!("unreadable ({e})"),
        Ok(s) if s.files == 0 && !s.indexed => "no supported code files here".to_string(),
        Ok(s) if !s.indexed => format!("not built yet, {}; any code command builds it", count(s.files, "file")),
        Ok(s) if s.stale > 0 => {
            format!("{}, {} reindex on the next code command", count(s.symbols, "symbol"), count(s.stale, "changed file"))
        }
        Ok(s) => format!("{}, current", count(s.symbols, "symbol")),
    };
    let learnings = crate::engine::learnings::active_learnings(root).map_or(0, |l| l.len());
    let python_block = if crate::hooks::repo_python_block(root) { "on" } else { "off" };
    format!("[KNOT] repo    {}\n        index={index}  learnings={learnings}\n        python_block={python_block}\n", root.display())
}

/// The code graph for `root`, reparsing only files changed since the last load, so
/// every caller sees the working tree as it is without a separate build step.
pub fn load_graph(root: &Path) -> anyhow::Result<crate::ast::CodeGraph> {
    let (corpus, _) = load_corpus(root, Some(&index_root(root)))?;
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

/// The files most likely behind a failure's output, best first, at most `limit`. An
/// output that names no file in this repository gets none: an error from a
/// dependency or the system is not this repo's to guess at.
pub fn failure_files(root: &Path, output: &str, limit: usize) -> anyhow::Result<Vec<String>> {
    let (corpus, _) = load_corpus(root, Some(&index_root(root)))?;
    if paths::named_files(output, &repo_files(&corpus.graph)).is_empty() {
        return Ok(Vec::new());
    }

    let mut files: Vec<String> = Vec::new();
    for hit in rank(&corpus.graph, &corpus.index, output, LEXICAL_CANDIDATES, Ranking::Hybrid) {
        if files.len() == limit {
            break;
        }
        if !files.contains(&hit.file) {
            files.push(hit.file);
        }
    }
    Ok(files)
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
    fn a_file_with_more_matches_wins_a_tie() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "src/a.rs", "pub fn parse_token() {}\n");
        write(dir.path(), "src/b.rs", "pub fn parse_token() {}\npub fn token_size() {}\n");

        let hits = ask(dir.path(), None, "parse token", 5, Ranking::LexicalOnly).unwrap();
        assert_eq!(hits[0].pointer(), "src/b.rs:L1-L1");
    }

    #[test]
    fn matches_past_the_reranked_candidates_still_come_back_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let body: String = (0..70).map(|i| format!("pub fn parse_part_{i}() {{}}\n")).collect();
        write(dir.path(), "src/parts.rs", &body);

        let all = ask(dir.path(), None, "parse part", 100, Ranking::Hybrid).unwrap();
        let top = ask(dir.path(), None, "parse part", LEXICAL_CANDIDATES, Ranking::Hybrid).unwrap();
        assert_eq!(all.len(), 70);
        let names = |hits: &[AskHit]| hits.iter().map(|h| h.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&all[..LEXICAL_CANDIDATES]), names(&top), "the reranked head is unchanged");
        assert!(all.windows(2).all(|w| w[0].score >= w[1].score), "scores never rise down the list");
    }

    #[test]
    fn a_failing_test_named_in_the_query_brings_in_the_code_it_tests() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "gate/gate.go", "package gate\nfunc Verify() {}\n");
        write(dir.path(), "gate/gate_test.go", "package gate\nfunc TestPlaceholders() {}\n");
        write(dir.path(), "other/expected.go", "package other\nfunc ExpectedUser() {}\n");
        write(dir.path(), "other/alice.go", "package other\nfunc AliceUserFailure() {}\n");

        let trace = "--- FAIL: TestPlaceholders (0.00s)\n    gate_test.go:67: expected user to be alice";
        let hits = ask(dir.path(), None, trace, 5, Ranking::Hybrid).unwrap();
        let files: Vec<&str> = hits.iter().map(|h| h.file.as_str()).collect();
        assert_eq!(files.first(), Some(&"gate/gate.go"), "{files:?}");
    }

    #[test]
    fn an_edited_file_named_in_the_query_brings_in_its_test() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "src/render.ts", "export function renderPage() {}\n");
        write(dir.path(), "src/render.test.ts", "function checksLayout() {}\n");
        for name in ["page", "title", "header", "footer"] {
            write(dir.path(), &format!("src/{name}.ts"), &format!("export function render{name}Page() {{}}\n"));
        }

        let hits = ask(dir.path(), None, "changed src/render.ts: render the page", 3, Ranking::Hybrid).unwrap();
        let files: Vec<&str> = hits.iter().map(|h| h.file.as_str()).collect();
        assert!(files.contains(&"src/render.test.ts"), "{files:?}");
    }


    #[test]
    fn a_failure_naming_repo_files_gets_its_likely_files() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "gate/gate.go", "package gate\nfunc Verify() {}\n");
        write(dir.path(), "gate/gate_test.go", "package gate\nfunc TestPlaceholders() {}\n");
        write(dir.path(), "other/expected.go", "package other\nfunc ExpectedUser() {}\n");

        let failure = "--- FAIL: TestPlaceholders (0.00s)\n    gate_test.go:67: expected user to be alice";
        let files = failure_files(dir.path(), failure, 2).unwrap();
        assert_eq!(files.first().map(String::as_str), Some("gate/gate.go"));
        assert!(files.len() <= 2);

        let elsewhere = "Traceback (most recent call last):\n  File \"/usr/lib/python3/site.py\", line 3\nKeyError: user";
        assert!(failure_files(dir.path(), elsewhere, 2).unwrap().is_empty(), "no repo file named, no guess");
    }

    #[test]
    fn empty_or_unmatched_query_returns_nothing() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "src/a.rs", "pub fn alpha() {}\n");
        assert!(ask(dir.path(), None, "zzzz qqqq", 5, Ranking::Hybrid).unwrap().is_empty());
        assert!(ask(dir.path(), None, "", 5, Ranking::LexicalOnly).unwrap().is_empty());
    }

    #[test]
    fn repo_summary_says_whether_the_code_is_indexed() {
        let dir = tempfile::tempdir().unwrap();
        assert!(repo_summary(dir.path()).contains("no supported code files"));

        write(dir.path(), "src/a.rs", "pub fn alpha() {}\npub fn beta() {}\n");
        assert!(repo_summary(dir.path()).contains("not built yet, 1 file;"));

        load_graph(dir.path()).unwrap();
        let summary = repo_summary(dir.path());
        assert!(summary.contains("index=2 symbols, current") && summary.contains("learnings=0") && summary.contains("python_block=off"), "{summary}");

        write(dir.path(), "src/b.rs", "pub fn gamma() {}\n");
        assert!(repo_summary(dir.path()).contains("1 changed file reindex"));
    }
}

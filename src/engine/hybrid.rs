use std::collections::HashMap;
use std::path::Path;

const WEIGHT_VECTOR: f32 = 0.40;
const WEIGHT_LEXICAL: f32 = 0.35;
const WEIGHT_PAGERANK: f32 = 0.15;
const WEIGHT_UTILITY: f32 = 0.10;
const DEFAULT_CENTRALITY: f32 = 0.50;
const MIN_TOKEN_LEN: usize = 2;

/// Sanitize raw search tokens to only contain alphanumeric characters.
pub fn sanitize_token(token: &str) -> String {
    token
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_')
        .collect()
}

/// Build an SQLite FTS5 query string from user input.
///
/// Tokens are extracted, sanitized, and formatted as prefix match terms.
/// This prevents syntax errors from punctuation or operators in search queries.
pub fn build_fts_query(raw: &str) -> Option<String> {
    let tokens: Vec<String> = raw
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .map(sanitize_token)
        .filter(|t| t.len() >= MIN_TOKEN_LEN)
        .collect();

    if tokens.is_empty() {
        return None;
    }

    let clauses: Vec<String> = tokens
        .into_iter()
        .map(|token| format!("{token}*"))
        .collect();

    Some(clauses.join(" OR "))
}

/// Convert raw SQLite FTS5 BM25 rank into a normalized score between zero and one.
///
/// SQLite BM25 returns negative values where more negative indicates a closer match.
pub fn normalize_bm25(rank: f64) -> f32 {
    let rank_mag = rank.abs() as f32;
    if rank_mag <= 0.0 {
        return 0.0;
    }
    (0.50 + 0.50 * (rank_mag / (rank_mag + 1.0))).clamp(0.0, 1.0)
}

/// Scale PageRank centrality relative to average node weight in the graph.
pub fn normalize_pagerank(score: f64, total_nodes: usize) -> f32 {
    if total_nodes == 0 || score <= 0.0 {
        return DEFAULT_CENTRALITY;
    }
    let expected_average = 1.0 / (total_nodes as f64);
    let ratio = (score / expected_average) as f32;
    (ratio / (ratio + 1.0)).clamp(0.0, 1.0)
}

/// Map hybrid relevance score back to a distance value for ranking and confidence checks.
pub fn compute_effective_distance(hybrid_score: f32) -> f32 {
    (1.0 - hybrid_score).clamp(0.0, 1.0)
}

/// Look up centrality for a memory's verification target in PageRank scores keyed by
/// symbol id (`src/file.rs::Name`).
///
/// Targets may be absolute or repo-relative and may name a symbol with `#` or `::`.
/// A symbol target matches its own id (or a method qualified under it); a file target
/// takes the best score among the file's symbols.
pub fn resolve_centrality(
    scores: &HashMap<String, f64>,
    verification_path: Option<&str>,
    repo_root: Option<&Path>,
) -> f32 {
    let Some(target) = verification_path else {
        return DEFAULT_CENTRALITY;
    };
    if scores.is_empty() {
        return DEFAULT_CENTRALITY;
    }

    let (file, symbol) = crate::jitv::parse_target(target);
    let file = file.replace('\\', "/");
    let file = match repo_root.map(|r| r.to_string_lossy().replace('\\', "/")) {
        Some(root) => file.strip_prefix(&root).map(str::to_string).unwrap_or(file),
        None => file,
    };
    let file = file.trim_start_matches("./").trim_start_matches('/');
    let file_prefix = format!("{file}::");

    let best = scores
        .iter()
        .filter(|(key, _)| {
            let Some(name) = key.strip_prefix(&file_prefix) else {
                return false;
            };
            match symbol {
                None => true,
                Some(sym) => {
                    name == sym
                        || name.ends_with(&format!("::{sym}"))
                        || name.ends_with(&format!(".{sym}"))
                }
            }
        })
        .map(|(_, &score)| score)
        .fold(None, |acc: Option<f64>, s| Some(acc.map_or(s, |a| a.max(s))));

    match best {
        Some(score) => normalize_pagerank(score, scores.len()),
        None => DEFAULT_CENTRALITY,
    }
}

/// Combine vector similarity, lexical score, PageRank centrality, and node utility.
pub fn compute_hybrid_score(
    vector_sim: f32,
    lexical_score: f32,
    pagerank_score: f32,
    utility_score: f32,
) -> f32 {
    let total = (WEIGHT_VECTOR * vector_sim)
        + (WEIGHT_LEXICAL * lexical_score)
        + (WEIGHT_PAGERANK * pagerank_score)
        + (WEIGHT_UTILITY * utility_score);

    total.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_fts_query_sanitizes_punctuation() {
        let query = "StorageEngine::recall(query: &str) -> Result<Vec<T>>";
        let built = build_fts_query(query).unwrap();
        assert!(built.contains("StorageEngine*"));
        assert!(built.contains("recall*"));
        assert!(built.contains("Result*"));
        assert!(!built.contains('&'));
        assert!(!built.contains(':'));
    }

    #[test]
    fn test_normalize_bm25_bounds() {
        assert_eq!(normalize_bm25(0.0), 0.0);
        let weak = normalize_bm25(-0.5);
        let strong = normalize_bm25(-10.0);
        assert!(strong > weak);
        assert!(strong <= 1.0);
    }

    #[test]
    fn test_resolve_centrality_matches_symbol_and_prefix() {
        let mut scores = HashMap::new();
        scores.insert("src/lib.rs::CodeGraph".to_string(), 0.80);
        scores.insert("src/lib.rs::CodeGraph::build".to_string(), 0.60);
        scores.insert("src/lib.rs::helper".to_string(), 0.01);
        scores.insert("src/lib.rs.bak::helper".to_string(), 0.90);
        let root = Path::new("/repo");

        let direct = resolve_centrality(&scores, Some("src/lib.rs#CodeGraph"), None);
        let colon = resolve_centrality(&scores, Some("src/lib.rs::CodeGraph"), None);
        let method = resolve_centrality(&scores, Some("/repo/src/lib.rs#build"), Some(root));
        let file = resolve_centrality(&scores, Some("/repo/src/lib.rs"), Some(root));
        let leaf = resolve_centrality(&scores, Some("src/lib.rs#helper"), None);
        let unknown = resolve_centrality(&scores, Some("src/other.rs"), None);

        assert!(direct > DEFAULT_CENTRALITY);
        assert_eq!(direct, colon);
        assert!(method > DEFAULT_CENTRALITY);
        assert_eq!(file, direct, "file target takes its best symbol");
        assert!(leaf < direct);
        assert_eq!(unknown, DEFAULT_CENTRALITY);
    }

    #[test]
    fn test_hybrid_score_calculation() {
        let score = compute_hybrid_score(1.0, 1.0, 1.0, 1.0);
        assert!((score - 1.0).abs() < 1e-5);

        let distance = compute_effective_distance(score);
        assert!(distance < 0.01);
    }
}

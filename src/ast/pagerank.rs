use std::collections::HashMap;

use super::resolve::CodeGraph;
use super::types::SymbolNode;

const DAMPING_FACTOR: f64 = 0.85;
const MAX_ITERATIONS: usize = 25;
const CONVERGENCE_THRESHOLD: f64 = 1e-6;

/// Run PageRank over the call graph, teleporting to `teleport` (which must sum to 1).
///
/// Rank held by symbols with no callees is redistributed along the teleport vector
/// instead of leaking out of the graph.
fn run_pagerank(
    ids: &[String],
    callers: &HashMap<String, Vec<String>>,
    callees: &HashMap<String, Vec<String>>,
    teleport: &HashMap<String, f64>,
) -> HashMap<String, f64> {
    let mut scores = teleport.clone();
    for id in ids {
        scores.entry(id.clone()).or_insert(0.0);
    }

    for _ in 0..MAX_ITERATIONS {
        let dangling_mass: f64 = ids
            .iter()
            .filter(|id| callees.get(*id).is_none_or(|c| c.is_empty()))
            .map(|id| scores[id])
            .sum();

        let mut next_scores = HashMap::with_capacity(ids.len());
        let mut max_delta = 0.0f64;

        for id in ids {
            let incoming_sum: f64 = callers
                .get(id)
                .map(|incoming| {
                    incoming
                        .iter()
                        .map(|caller| {
                            let out_degree = callees.get(caller).map_or(1, |c| c.len().max(1)) as f64;
                            scores.get(caller).copied().unwrap_or(0.0) / out_degree
                        })
                        .sum()
                })
                .unwrap_or(0.0);

            let jump = teleport.get(id).copied().unwrap_or(0.0);
            let new_score = (1.0 - DAMPING_FACTOR) * jump + DAMPING_FACTOR * (incoming_sum + dangling_mass * jump);
            max_delta = max_delta.max((new_score - scores[id]).abs());
            next_scores.insert(id.clone(), new_score);
        }

        scores = next_scores;
        if max_delta < CONVERGENCE_THRESHOLD {
            break;
        }
    }

    scores
}

/// Global PageRank: every symbol is an equally likely starting point.
pub fn compute_pagerank(graph: &CodeGraph) -> HashMap<String, f64> {
    let ids: Vec<String> = graph.symbols.keys().cloned().collect();
    if ids.is_empty() {
        return HashMap::new();
    }

    let uniform = 1.0 / ids.len() as f64;
    let teleport: HashMap<String, f64> = ids.iter().map(|id| (id.clone(), uniform)).collect();
    run_pagerank(&ids, &graph.callers, &graph.callees, &teleport)
}

/// Personalized PageRank: random walks restart only at `seeds`, so scores measure
/// how central each symbol is to the seeded part of the graph rather than the whole repo.
/// Unknown seeds are ignored; with no known seeds this falls back to global PageRank.
pub fn compute_personalized_pagerank(graph: &CodeGraph, seeds: &[String]) -> HashMap<String, f64> {
    let weights: HashMap<String, f64> = seeds.iter().map(|s| (s.clone(), 1.0)).collect();
    compute_weighted_pagerank(graph, &weights)
}

/// Personalized PageRank with a restart weight per seed, so stronger seeds pull
/// more rank toward their neighborhood. Weights need not sum to one.
pub fn compute_weighted_pagerank(graph: &CodeGraph, seed_weights: &HashMap<String, f64>) -> HashMap<String, f64> {
    let known: Vec<(&String, f64)> = seed_weights
        .iter()
        .filter(|(id, w)| graph.symbols.contains_key(*id) && **w > 0.0)
        .map(|(id, w)| (id, *w))
        .collect();
    let total: f64 = known.iter().map(|(_, w)| w).sum();
    if known.is_empty() || total <= 0.0 {
        return compute_pagerank(graph);
    }

    let ids: Vec<String> = graph.symbols.keys().cloned().collect();
    let teleport: HashMap<String, f64> = known.into_iter().map(|(id, w)| (id.clone(), w / total)).collect();
    run_pagerank(&ids, &graph.callers, &graph.callees, &teleport)
}

pub fn top_hubs<'a>(graph: &'a CodeGraph, limit: usize) -> Vec<(&'a SymbolNode, f64)> {
    let scores = compute_pagerank(graph);
    let mut ranked: Vec<(&'a SymbolNode, f64)> = graph
        .symbols
        .values()
        .filter_map(|sym| {
            let score = scores.get(&sym.id).copied()?;
            Some((sym, score))
        })
        .collect();

    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    ranked.truncate(limit);
    ranked
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::types::SymbolKind;

    fn make_test_sym(id: &str, outgoing: Vec<&str>) -> SymbolNode {
        SymbolNode {
            id: id.to_string(),
            file: "test.rs".to_string(),
            name: id.to_string(),
            kind: SymbolKind::Function,
            signature: format!("fn {id}()"),
            span: (1, 5),
            crux: String::new(),
            hash: "hash".to_string(),
            outgoing_calls: outgoing.into_iter().map(String::from).collect(),
        }
    }

    #[test]
    fn test_pagerank_favors_central_hub() {
        let mut graph = CodeGraph::default();

        let s_hub = make_test_sym("core_utility", vec![]);
        let s_a = make_test_sym("caller_a", vec!["core_utility"]);
        let s_b = make_test_sym("caller_b", vec!["core_utility"]);
        let s_c = make_test_sym("caller_c", vec!["core_utility"]);

        graph.symbols.insert("core_utility".into(), s_hub);
        graph.symbols.insert("caller_a".into(), s_a);
        graph.symbols.insert("caller_b".into(), s_b);
        graph.symbols.insert("caller_c".into(), s_c);

        graph.callers.insert(
            "core_utility".into(),
            vec!["caller_a".into(), "caller_b".into(), "caller_c".into()],
        );
        graph.callees.insert("caller_a".into(), vec!["core_utility".into()]);
        graph.callees.insert("caller_b".into(), vec!["core_utility".into()]);
        graph.callees.insert("caller_c".into(), vec!["core_utility".into()]);

        let hubs = top_hubs(&graph, 4);
        assert_eq!(hubs[0].0.id, "core_utility");
        assert!(hubs[0].1 > hubs[1].1);

        let total: f64 = compute_pagerank(&graph).values().sum();
        assert!((total - 1.0).abs() < 1e-6, "rank must not leak from dangling nodes: {total}");
    }

    #[test]
    fn test_personalized_pagerank_follows_seed() {
        let mut graph = CodeGraph::default();
        for (id, out) in [
            ("a", vec!["a_helper"]),
            ("a_helper", vec![]),
            ("b", vec!["b_helper"]),
            ("b_helper", vec![]),
        ] {
            graph.symbols.insert(id.into(), make_test_sym(id, out.clone()));
            for target in out {
                graph.callees.entry(id.into()).or_default().push(target.into());
                graph.callers.entry(target.into()).or_default().push(id.into());
            }
        }

        let scores = compute_personalized_pagerank(&graph, &["a".to_string()]);
        assert!(scores["a_helper"] > scores["b_helper"]);
        assert!(scores["a"] > scores["b"]);

        let total: f64 = scores.values().sum();
        assert!((total - 1.0).abs() < 1e-6);

        let fallback = compute_personalized_pagerank(&graph, &["missing".to_string()]);
        assert_eq!(fallback, compute_pagerank(&graph));
    }
}

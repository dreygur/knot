use std::collections::HashMap;

use super::resolve::CodeGraph;
use super::types::SymbolNode;

const DAMPING_FACTOR: f64 = 0.85;
const MAX_ITERATIONS: usize = 25;
const CONVERGENCE_THRESHOLD: f64 = 1e-6;

fn initialize_scores(ids: &[String]) -> (HashMap<String, f64>, f64) {
    let n = ids.len() as f64;
    let initial_score = if n > 0.0 { 1.0 / n } else { 0.0 };
    let mut scores = HashMap::with_capacity(ids.len());
    for id in ids {
        scores.insert(id.clone(), initial_score);
    }
    (scores, n)
}

fn step_pagerank(
    ids: &[String],
    current_scores: &HashMap<String, f64>,
    callers: &HashMap<String, Vec<String>>,
    callees: &HashMap<String, Vec<String>>,
    n: f64,
) -> (HashMap<String, f64>, f64) {
    let base_score = (1.0 - DAMPING_FACTOR) / n;
    let mut next_scores = HashMap::with_capacity(ids.len());
    let mut max_delta = 0.0f64;

    for id in ids {
        let mut incoming_sum = 0.0f64;
        if let Some(incoming_nodes) = callers.get(id) {
            for incoming_id in incoming_nodes {
                let incoming_score = current_scores.get(incoming_id).copied().unwrap_or(0.0);
                let out_degree = callees
                    .get(incoming_id)
                    .map(|v| v.len())
                    .unwrap_or(1)
                    .max(1) as f64;
                incoming_sum += incoming_score / out_degree;
            }
        }

        let new_score = base_score + (DAMPING_FACTOR * incoming_sum);
        let old_score = current_scores.get(id).copied().unwrap_or(0.0);
        let delta = (new_score - old_score).abs();
        if delta > max_delta {
            max_delta = delta;
        }

        next_scores.insert(id.clone(), new_score);
    }

    (next_scores, max_delta)
}

pub fn compute_pagerank(graph: &CodeGraph) -> HashMap<String, f64> {
    let ids: Vec<String> = graph.symbols.keys().cloned().collect();
    if ids.is_empty() {
        return HashMap::new();
    }

    let (mut scores, n) = initialize_scores(&ids);

    for _ in 0..MAX_ITERATIONS {
        let (next_scores, max_delta) = step_pagerank(&ids, &scores, &graph.callers, &graph.callees, n);
        scores = next_scores;
        if max_delta < CONVERGENCE_THRESHOLD {
            break;
        }
    }

    scores
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
    }
}

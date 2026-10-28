//! The data the `knot viz` page lays out and draws: symbols, call edges as
//! indices, folders, PageRank and the learnings that touch each symbol's file.

use std::collections::{BTreeSet, HashMap};

use serde::Serialize;

use crate::ast::pagerank::compute_pagerank;
use crate::ast::resolve::CodeGraph;
use crate::ast::types::SymbolNode;
use crate::engine::learnings::RepoLearning;

/// Everything the page draws, serialized into its inline script. Nodes are sorted by
/// id, so the same graph always gives the same JSON and the same cached layout.
#[derive(Serialize)]
pub(crate) struct VizData {
    pub(crate) folders: Vec<String>,
    pub(crate) nodes: Vec<VizNode>,
    /// Calls as `(caller, callee)` indices into `nodes`.
    pub(crate) edges: Vec<(usize, usize)>,
    pub(crate) learnings: Vec<VizLearning>,
}

/// One symbol on the map. The JSON names are one or two letters because a large
/// repo has tens of thousands of nodes, all inlined into the page.
#[derive(Serialize)]
pub(crate) struct VizNode {
    pub(crate) id: String,
    #[serde(rename = "l")]
    pub(crate) name: String,
    #[serde(rename = "f")]
    pub(crate) file: String,
    #[serde(rename = "ln")]
    pub(crate) line: usize,
    #[serde(rename = "s")]
    pub(crate) signature: String,
    /// Index into `VizData::folders`.
    #[serde(rename = "fo")]
    pub(crate) folder: usize,
    #[serde(rename = "k")]
    pub(crate) kind: String,
    /// PageRank, rounded to six places to keep the JSON short.
    #[serde(rename = "pr")]
    pub(crate) rank: f64,
    /// Indices into `VizData::learnings` of the learnings touching this symbol's file.
    #[serde(rename = "le")]
    pub(crate) learnings: Vec<usize>,
}

/// The part of a learning the inspector shows.
#[derive(Serialize)]
pub(crate) struct VizLearning {
    pub(crate) title: String,
    pub(crate) author: String,
    pub(crate) date: String,
}

/// The first two path segments of a file, or the first when the path has
/// exactly two, or "(root)" for a bare file name.
fn folder_of(file: &str) -> String {
    let parts: Vec<&str> = file.split('/').collect();
    match parts.len() {
        0 | 1 => "(root)".to_string(),
        2 => parts[0].to_string(),
        _ => parts[..2].join("/"),
    }
}

/// Same rule as the hook: a learning touches a file when a `touches` entry,
/// minus any `#fragment` and leading `./`, is that file's path.
fn touches(learning: &RepoLearning, file: &str) -> bool {
    learning.frontmatter.touches.iter().any(|t| {
        let t = t.replace('\\', "/");
        let path = t.split('#').next().unwrap_or("").trim_start_matches("./");
        path == file
    })
}

/// Turns the code graph and the active learnings into what the page draws. Calls to
/// symbols outside the graph and calls to self are dropped.
pub(crate) fn viz_data(graph: &CodeGraph, learnings: &[RepoLearning]) -> VizData {
    let rank = compute_pagerank(graph);
    let mut symbols: Vec<&SymbolNode> = graph.symbols.values().collect();
    symbols.sort_by(|a, b| a.id.cmp(&b.id));

    let mut folders: Vec<String> = symbols.iter().map(|s| folder_of(&s.file)).collect();
    folders.sort();
    folders.dedup();

    let index: HashMap<&str, usize> = symbols.iter().enumerate().map(|(i, s)| (s.id.as_str(), i)).collect();

    let nodes = symbols
        .iter()
        .map(|s| {
            let folder = folder_of(&s.file);
            let file = s.file.replace('\\', "/");
            VizNode {
                id: s.id.clone(),
                name: s.name.clone(),
                file: s.file.clone(),
                line: s.span.0,
                signature: s.signature.clone(),
                folder: folders.binary_search(&folder).unwrap_or(0),
                kind: s.kind.as_str().to_string(),
                rank: (rank.get(&s.id).copied().unwrap_or(0.0) * 1e6).round() / 1e6,
                learnings: (0..learnings.len()).filter(|&i| touches(&learnings[i], &file)).collect(),
            }
        })
        .collect();

    let mut edges = BTreeSet::new();
    for (from, tos) in &graph.callees {
        let Some(&a) = index.get(from.as_str()) else { continue };
        for to in tos {
            let Some(&b) = index.get(to.as_str()) else { continue };
            if a == b {
                continue;
            }
            edges.insert((a, b));
        }
    }

    VizData {
        folders,
        nodes,
        edges: edges.into_iter().collect(),
        learnings: learnings
            .iter()
            .map(|l| VizLearning {
                title: l.frontmatter.title.clone(),
                author: l.frontmatter.author.clone(),
                date: l.frontmatter.date.clone(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::types::{SymbolKind, SymbolNode};
    use crate::engine::learnings::LearningFrontmatter;

    fn sym(file: &str, name: &str, kind: SymbolKind) -> SymbolNode {
        SymbolNode {
            id: format!("{file}::{name}"),
            file: file.into(),
            name: name.into(),
            kind,
            signature: format!("fn {name}()"),
            span: (7, 9),
            crux: String::new(),
            hash: String::new(),
            outgoing_calls: vec![],
        }
    }

    fn graph(symbols: Vec<SymbolNode>, calls: &[(&str, &str)]) -> CodeGraph {
        let mut g = CodeGraph::default();
        for s in symbols {
            g.symbols.insert(s.id.clone(), s);
        }
        for (from, to) in calls {
            g.callees.entry((*from).into()).or_default().push((*to).into());
        }
        g
    }

    fn learning(title: &str, touches: &[&str]) -> RepoLearning {
        RepoLearning {
            frontmatter: LearningFrontmatter {
                title: title.into(),
                author: "Maya".into(),
                date: "2026-10-09".into(),
                touches: touches.iter().map(|t| t.to_string()).collect(),
                ..Default::default()
            },
            body: String::new(),
            file_path: "x.md".into(),
        }
    }

    #[test]
    fn folder_of_matches_the_prototype() {
        assert_eq!(folder_of("src/cli/mod.rs"), "src/cli");
        assert_eq!(folder_of("src/lib.rs"), "src");
        assert_eq!(folder_of("main.rs"), "(root)");
    }

    #[test]
    fn edges_are_indices_without_self_or_duplicate_calls() {
        let g = graph(
            vec![sym("a.rs", "a", SymbolKind::Function), sym("b.rs", "b", SymbolKind::Function)],
            &[("a.rs::a", "b.rs::b"), ("a.rs::a", "b.rs::b"), ("a.rs::a", "a.rs::a"), ("a.rs::a", "ghost::g")],
        );
        let d = viz_data(&g, &[]);
        assert_eq!(d.nodes[0].id, "a.rs::a");
        assert_eq!(d.nodes[1].id, "b.rs::b");
        assert_eq!(d.edges, vec![(0, 1)]);
    }

    #[test]
    fn learnings_attach_by_touched_file() {
        let g = graph(
            vec![sym("src/a.rs", "a", SymbolKind::Function), sym("src/b.rs", "b", SymbolKind::Function)],
            &[],
        );
        let ls = [learning("other", &["zzz.rs"]), learning("about a", &["./src/a.rs#L3", "src\\a.rs"])];
        let d = viz_data(&g, &ls);
        assert_eq!(d.learnings.len(), 2);
        assert_eq!(d.learnings[1].title, "about a");
        assert_eq!(d.nodes[0].learnings, vec![1]);
        assert!(d.nodes[1].learnings.is_empty());
    }

    #[test]
    fn kind_comes_from_symbol_kind() {
        let g = graph(vec![sym("a.rs", "S", SymbolKind::Struct)], &[]);
        let d = viz_data(&g, &[]);
        assert_eq!(d.nodes[0].kind, "struct");
        assert_eq!(d.nodes[0].name, "S");
        assert_eq!(d.nodes[0].line, 7);
        assert_eq!(d.nodes[0].signature, "fn S()");
        assert_eq!(d.folders, vec!["(root)"]);
        assert_eq!(d.nodes[0].folder, 0);
    }

    #[test]
    fn wire_names_stay_short() {
        let g = graph(vec![sym("src/a.rs", "a", SymbolKind::Function)], &[]);
        let json = serde_json::to_string(&viz_data(&g, &[learning("l", &["src/a.rs"])])).unwrap();
        let node = r#"{"id":"src/a.rs::a","l":"a","f":"src/a.rs","ln":7,"s":"fn a()","fo":0,"k":"function","pr":"#;
        assert!(json.contains(node), "{json}");
        assert!(json.contains(r#""le":[0]}"#), "{json}");
    }

    #[test]
    fn empty_graph_data() {
        let d = viz_data(&CodeGraph::default(), &[]);
        assert!(d.folders.is_empty() && d.nodes.is_empty() && d.edges.is_empty() && d.learnings.is_empty());
    }

    #[test]
    fn output_is_deterministic() {
        let g = graph(
            vec![
                sym("src/x/a.rs", "a", SymbolKind::Function),
                sym("src/y/b.rs", "b", SymbolKind::Method),
                sym("c.rs", "c", SymbolKind::Enum),
            ],
            &[("src/x/a.rs::a", "src/y/b.rs::b"), ("c.rs::c", "src/x/a.rs::a")],
        );
        let ls = [learning("l", &["c.rs"])];
        let one = serde_json::to_string(&viz_data(&g, &ls)).unwrap();
        let two = serde_json::to_string(&viz_data(&g, &ls)).unwrap();
        assert_eq!(one, two);
        let d = viz_data(&g, &ls);
        assert_eq!(d.folders, vec!["(root)", "src/x", "src/y"]);
    }
}

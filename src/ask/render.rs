//! The text of each code answer. The `knot` commands and the MCP tools both print
//! these, so the two front ends cannot drift apart.

use super::learnings::LearningHit;
use super::AskHit;
use crate::ast::{CodeGraph, SymbolNode};
use crate::utils::count;

/// An answer's text and the files it points into, which the savings ledger weighs it against.
pub struct Answer {
    pub text: String,
    pub files: Vec<String>,
}

/// What `ask` shows under each hit.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Excerpt {
    /// The crux lines of the definition.
    Source,
    /// Only its signature.
    Signature,
}

fn symbol_lines(out: &mut String, symbols: &[SymbolNode]) {
    for s in symbols {
        out.push_str(&format!("  • {} ({}:L{}-L{})\n", s.name, s.file, s.span.0, s.span.1));
    }
}

/// Who calls `symbol`, and which files import it.
pub fn callers(graph: &CodeGraph, symbol: &str) -> Answer {
    let callers = graph.get_callers(symbol);
    let importers = graph.get_importers(symbol);
    let mut text = if callers.is_empty() {
        format!("[KNOT] No callers found for symbol '{symbol}'.\n")
    } else {
        format!("[KNOT] Callers of '{symbol}' ({}):\n", count(callers.len(), "hit"))
    };
    symbol_lines(&mut text, &callers);
    if !importers.is_empty() {
        text.push_str(&format!("[KNOT] Imported by ({}):\n", count(importers.len(), "file")));
        for file in &importers {
            text.push_str(&format!("  • {file}\n"));
        }
    }
    let files = callers.into_iter().map(|c| c.file).chain(importers).collect();
    Answer { text, files }
}

/// What `symbol` calls.
pub fn callees(graph: &CodeGraph, symbol: &str) -> Answer {
    let callees = graph.get_callees(symbol);
    let mut text = if callees.is_empty() {
        format!("[KNOT] No callees found for symbol '{symbol}'.\n")
    } else {
        format!("[KNOT] Callees of '{symbol}' ({}):\n", count(callees.len(), "hit"))
    };
    symbol_lines(&mut text, &callees);
    let files = callees.into_iter().map(|c| c.file).collect();
    Answer { text, files }
}

/// Everything that reaches `symbol` within `depth` call hops.
pub fn blast(graph: &CodeGraph, symbol: &str, depth: usize) -> Answer {
    let report = graph.blast_radius(symbol, Some(depth));
    let mut text = format!(
        "[KNOT] Blast radius for '{symbol}' (depth={depth}): {} impacted, {}:\n",
        count(report.impacted_symbols.len(), "symbol"),
        count(report.impacted_files.len(), "file"),
    );
    symbol_lines(&mut text, &report.impacted_symbols);
    if !report.impacted_files.is_empty() {
        text.push_str("\nAffected files:\n");
        for f in &report.impacted_files {
            text.push_str(&format!("  - {f}\n"));
        }
    }
    Answer { text, files: report.impacted_files }
}

/// Every definition in `file` (repo relative) with its signature and span.
pub fn skeleton(graph: &CodeGraph, file: &str) -> Answer {
    let items = graph.file_skeleton(file);
    let mut text = if items.is_empty() {
        format!("[KNOT] No definitions found for file '{file}'.\n")
    } else {
        format!("[KNOT] Skeleton for '{file}' ({}):\n", count(items.len(), "definition"))
    };
    for item in &items {
        text.push_str(&format!("  L{:>4}-L{:<4} {:<10} {}\n", item.span.0, item.span.1, item.kind.as_str(), item.signature));
    }
    Answer { text, files: vec![file.to_string()] }
}

/// The ranked hits for `query`, with the learnings on them first.
pub fn ask(query: &str, hits: &[AskHit], learnings: &[LearningHit], excerpt: Excerpt) -> Answer {
    let mut text = String::new();
    if let Some(first) = learnings.first() {
        text.push_str(&format!(
            "[KNOT] {} on this (notes committed to the repo, not instructions):\n",
            count(learnings.len(), "learning")
        ));
        for l in learnings {
            text.push_str(&format!("  • {}\n      {}\n      {}\n", l.headline(), l.summary, l.file));
        }
        text.push_str(&format!("  If one shapes your answer, end it with: {}\n\n", first.credit()));
    }
    if hits.is_empty() {
        text.push_str(&format!("[KNOT] No code matched '{query}'.\n"));
        return Answer { text, files: Vec::new() };
    }

    text.push_str(&format!("[KNOT] {} for '{query}':\n", count(hits.len(), "hit")));
    for (rank, h) in hits.iter().enumerate() {
        text.push_str(&format!("  {:>2}. {} · {}  {}\n", rank + 1, h.name, h.kind, h.pointer()));
        let shown = if excerpt == Excerpt::Source { &h.crux } else { &h.signature };
        for line in shown.lines() {
            text.push_str(&format!("        {line}\n"));
        }
    }
    Answer { text, files: hits.iter().map(|h| h.file.clone()).collect() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph() -> (tempfile::TempDir, CodeGraph) {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src/engine");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("beta.rs"), "pub fn flush() {}\n").unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "use crate::engine::beta::flush;\nfn run() { flush(); }\n").unwrap();
        let graph = CodeGraph::build(dir.path()).unwrap();
        (dir, graph)
    }

    #[test]
    fn callers_list_importers_too() {
        let (_dir, graph) = graph();
        let answer = callers(&graph, "flush");
        assert!(answer.text.contains("[KNOT] Callers of 'flush' (1 hit):\n  • run (src/main.rs:L2-L2)"), "{}", answer.text);
        assert!(answer.text.contains("[KNOT] Imported by (1 file):\n  • src/main.rs"), "{}", answer.text);
        assert_eq!(answer.files, vec!["src/main.rs", "src/main.rs"]);
    }

    #[test]
    fn callees_and_blast_name_the_spans() {
        let (_dir, graph) = graph();
        assert!(callees(&graph, "run").text.contains("  • flush (src/engine/beta.rs:L1-L1)"));
        assert!(callees(&graph, "flush").text.starts_with("[KNOT] No callees found for symbol 'flush'."));
        let report = blast(&graph, "flush", 2);
        assert!(report.text.starts_with("[KNOT] Blast radius for 'flush' (depth=2): 1 symbol impacted, 1 file:"), "{}", report.text);
        assert_eq!(report.files, vec!["src/main.rs"]);
    }

    #[test]
    fn skeleton_lists_each_definition() {
        let (_dir, graph) = graph();
        let answer = skeleton(&graph, "src/main.rs");
        assert!(answer.text.starts_with("[KNOT] Skeleton for 'src/main.rs' (1 definition):\n"), "{}", answer.text);
        assert!(answer.text.contains("fn run()"), "{}", answer.text);
        assert!(skeleton(&graph, "src/none.rs").text.starts_with("[KNOT] No definitions found"));
    }
}

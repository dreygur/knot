use std::path::Path;
use std::time::Instant;

use knot::ast::{compute_pagerank, CodeGraph};

const MAX_INDEX_TIME_MILLIS: u128 = 5000;
const MAX_PAGERANK_TIME_MILLIS: u128 = 500;

fn target_dir() -> &'static str {
    if Path::new("reference/trail-code/src").exists() {
        "reference/trail-code/src"
    } else {
        "src"
    }
}

fn estimate_tokens(chars: usize) -> usize {
    chars / 4
}

#[test]
fn test_benchmark_ast_indexing_and_pagerank() {
    let dir_path = target_dir();
    let root = Path::new(dir_path);

    println!("\n[BENCHMARK] Target Directory: {dir_path}");

    let start_index = Instant::now();
    let graph = CodeGraph::build(root).expect("CodeGraph build failed");
    let index_duration = start_index.elapsed();

    println!(
        "[BENCHMARK] Indexed {} files in {:.2?} ({} symbols extracted)",
        graph.file_to_ids.len(),
        index_duration,
        graph.symbols.len()
    );

    assert!(!graph.symbols.is_empty());
    assert!(
        index_duration.as_millis() < MAX_INDEX_TIME_MILLIS,
        "Indexing took too long: {} ms",
        index_duration.as_millis()
    );

    let start_pagerank = Instant::now();
    let scores = compute_pagerank(&graph);
    let pagerank_duration = start_pagerank.elapsed();

    println!(
        "[BENCHMARK] PageRank computed for {} nodes in {:.2?}",
        scores.len(),
        pagerank_duration
    );

    assert_eq!(scores.len(), graph.symbols.len());
    assert!(
        pagerank_duration.as_millis() < MAX_PAGERANK_TIME_MILLIS,
        "PageRank took too long: {} ms",
        pagerank_duration.as_millis()
    );
}

#[test]
fn test_benchmark_skeleton_token_efficiency() {
    let dir_path = target_dir();
    let root = Path::new(dir_path);
    let graph = CodeGraph::build(root).expect("CodeGraph build failed");

    let mut total_source_chars = 0usize;
    let mut total_skeleton_chars = 0usize;
    let mut files_measured = 0usize;

    for file_rel in graph.file_to_ids.keys() {
        let full_path = if dir_path == "reference/trail-code/src" {
            Path::new("reference/trail-code/src").join(file_rel)
        } else {
            Path::new(file_rel).to_path_buf()
        };

        if let Ok(source) = std::fs::read_to_string(&full_path) {
            let skeleton = graph.file_skeleton(file_rel);
            let skeleton_text: String = skeleton
                .iter()
                .map(|item| format!("{} {}\n", item.kind.as_str(), item.signature))
                .collect();

            total_source_chars += source.len();
            total_skeleton_chars += skeleton_text.len();
            files_measured += 1;
        }
    }

    if files_measured > 0 && total_source_chars > 0 {
        let source_tokens = estimate_tokens(total_source_chars);
        let skeleton_tokens = estimate_tokens(total_skeleton_chars);
        let compression_ratio =
            (1.0 - (total_skeleton_chars as f64 / total_source_chars as f64)) * 100.0;

        println!(
            "\n[BENCHMARK] Skeleton Token Efficiency across {} files:",
            files_measured
        );
        println!("  Source payload:   {} chars (~{} tokens)", total_source_chars, source_tokens);
        println!("  Skeleton payload: {} chars (~{} tokens)", total_skeleton_chars, skeleton_tokens);
        println!("  Token reduction:  {:.1}%", compression_ratio);

        assert!(
            compression_ratio > 50.0,
            "Skeleton should achieve at least 50% token reduction, got {:.1}%",
            compression_ratio
        );
    }
}

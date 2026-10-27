//! `knot grep`: every line matching a pattern across the indexed source files, grouped
//! by the innermost symbol that contains it and ordered by how much code calls that
//! symbol. Ranked search answers "where is X"; this answers "every place X appears".

use anyhow::{Context, Result};
use rayon::prelude::*;
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;

use super::load_corpus;
use crate::ast::resolve::list_supported_files;
use crate::ast::CodeGraph;

/// Matched lines longer than this are cut, so one minified line cannot flood the output.
const MAX_LINE_CHARS: usize = 200;

#[derive(Debug, Clone, Default)]
pub struct GrepOptions {
    pub pattern: String,
    pub ignore_case: bool,
    /// Treat the pattern as literal text rather than a regex.
    pub fixed: bool,
    /// Only files at or under this repo-relative path.
    pub within: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GrepLine {
    pub line: usize,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct GrepGroup {
    pub file: String,
    /// The enclosing symbol, or `None` for lines at the file's top level.
    pub symbol: Option<String>,
    pub kind: Option<String>,
    pub span: Option<(usize, usize)>,
    /// Symbols that call the enclosing symbol; groups are ordered by this.
    pub callers: usize,
    pub lines: Vec<GrepLine>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GrepReport {
    pub pattern: String,
    pub hits: usize,
    pub files_matched: usize,
    pub files_searched: usize,
    pub groups: Vec<GrepGroup>,
}

fn build_regex(opts: &GrepOptions) -> Result<regex::Regex> {
    let pattern = if opts.fixed { regex::escape(&opts.pattern) } else { opts.pattern.clone() };
    regex::RegexBuilder::new(&pattern)
        .case_insensitive(opts.ignore_case)
        .build()
        .with_context(|| format!("'{}' is not a valid regex; pass --fixed to search for it literally", opts.pattern))
}

fn cut(text: &str) -> String {
    let trimmed = text.trim();
    match trimmed.char_indices().nth(MAX_LINE_CHARS) {
        Some((at, _)) => format!("{}...", &trimmed[..at]),
        None => trimmed.to_string(),
    }
}

/// The smallest symbol in `file` whose span contains `line`.
fn enclosing<'g>(graph: &'g CodeGraph, file: &str, line: usize) -> Option<&'g crate::ast::SymbolNode> {
    graph
        .file_to_ids
        .get(file)?
        .iter()
        .filter_map(|id| graph.symbols.get(id))
        .filter(|s| s.span.0 <= line && line <= s.span.1)
        .min_by_key(|s| (s.span.1 - s.span.0, s.span.0))
}

/// Search every indexed file under `root` for `opts.pattern`.
pub fn grep(root: &Path, cache_root: Option<&Path>, opts: &GrepOptions) -> Result<GrepReport> {
    let regex = build_regex(opts)?;
    let within = opts
        .within
        .as_deref()
        .map(|w| w.trim_start_matches("./").trim_end_matches('/').to_string())
        .filter(|w| !w.is_empty() && w != ".");

    let files: Vec<String> = list_supported_files(root)?
        .iter()
        .map(|p| p.strip_prefix(root).unwrap_or(p).to_string_lossy().replace('\\', "/"))
        .filter(|rel| within.as_deref().is_none_or(|w| rel == w || rel.starts_with(&format!("{w}/"))))
        .collect();

    let matches: Vec<(String, Vec<GrepLine>)> = files
        .par_iter()
        .filter_map(|rel| {
            let source = std::fs::read_to_string(root.join(rel)).ok()?;
            let lines: Vec<GrepLine> = source
                .lines()
                .enumerate()
                .filter(|(_, l)| regex.is_match(l))
                .map(|(i, l)| GrepLine { line: i + 1, text: cut(l) })
                .collect();
            (!lines.is_empty()).then(|| (rel.clone(), lines))
        })
        .collect();

    let (corpus, _) = load_corpus(root, cache_root)?;
    let graph = &corpus.graph;

    let mut groups: HashMap<(String, Option<String>), GrepGroup> = HashMap::new();
    let mut hits = 0;
    for (file, lines) in &matches {
        for hit in lines {
            hits += 1;
            let symbol = enclosing(graph, file, hit.line);
            let key = (file.clone(), symbol.map(|s| s.id.clone()));
            groups
                .entry(key)
                .or_insert_with(|| GrepGroup {
                    file: file.clone(),
                    symbol: symbol.map(|s| s.name.clone()),
                    kind: symbol.map(|s| s.kind.as_str().to_string()),
                    span: symbol.map(|s| s.span),
                    callers: symbol.and_then(|s| graph.callers.get(&s.id)).map_or(0, Vec::len),
                    lines: Vec::new(),
                })
                .lines
                .push(hit.clone());
        }
    }

    let mut groups: Vec<GrepGroup> = groups.into_values().collect();
    for g in &mut groups {
        g.lines.sort_by_key(|l| l.line);
    }
    groups.sort_by(|a, b| {
        b.callers
            .cmp(&a.callers)
            .then(b.lines.len().cmp(&a.lines.len()))
            .then_with(|| a.file.cmp(&b.file))
            .then(a.lines[0].line.cmp(&b.lines[0].line))
    });

    Ok(GrepReport {
        pattern: opts.pattern.clone(),
        hits,
        files_matched: matches.len(),
        files_searched: files.len(),
        groups,
    })
}

/// Plain text for the CLI and the MCP tool.
pub fn format_report(report: &GrepReport) -> String {
    if report.hits == 0 {
        return format!(
            "No lines match '{}' in {} indexed file(s). Every indexed file was searched; try a shorter pattern or -i.",
            report.pattern, report.files_searched
        );
    }
    let mut out = format!(
        "'{}': {} hit(s) in {} place(s) across {} file(s) (searched {} indexed files)\n",
        report.pattern,
        report.hits,
        report.groups.len(),
        report.files_matched,
        report.files_searched
    );
    for g in &report.groups {
        out.push('\n');
        match (&g.symbol, &g.kind, g.span) {
            (Some(name), Some(kind), Some((start, end))) => {
                out.push_str(&format!("{name} · {kind} · {}:L{start}-L{end} · {} caller(s)\n", g.file, g.callers));
            }
            _ => out.push_str(&format!("{} · top level\n", g.file)),
        }
        for l in &g.lines {
            out.push_str(&format!("  L{}: {}\n", l.line, l.text));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src/net")).unwrap();
        std::fs::write(
            root.join("src/net/client.rs"),
            "const DEFAULT_TIMEOUT: u64 = 30;\n\
             pub fn send(timeout: u64) {\n    wait(timeout);\n}\n\
             pub fn wait(timeout: u64) {}\n",
        )
        .unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() { send(5); send(6); }\n").unwrap();
        std::fs::write(root.join("README.md"), "timeout docs are not indexed\n").unwrap();
        dir
    }

    fn opts(pattern: &str) -> GrepOptions {
        GrepOptions { pattern: pattern.into(), ..Default::default() }
    }

    #[test]
    fn groups_hits_by_enclosing_symbol_and_orders_by_callers() {
        let dir = repo();
        let report = grep(dir.path(), None, &GrepOptions { ignore_case: true, ..opts("timeout") }).unwrap();

        assert_eq!(report.files_searched, 2, "only indexed source files");
        assert_eq!(report.hits, 4);
        let names: Vec<Option<&str>> = report.groups.iter().map(|g| g.symbol.as_deref()).collect();
        // send has a caller (main), wait has one (send); send holds more hits so it leads
        assert_eq!(names, vec![Some("send"), Some("wait"), None]);
        assert_eq!(report.groups[0].lines.iter().map(|l| l.line).collect::<Vec<_>>(), vec![2, 3]);
        assert_eq!(report.groups[2].lines[0].text, "const DEFAULT_TIMEOUT: u64 = 30;");

        let text = format_report(&report);
        assert!(text.contains("send · function · src/net/client.rs:L2-L4 · 1 caller(s)"), "{text}");
        assert!(text.contains("src/net/client.rs · top level"), "{text}");
    }

    #[test]
    fn fixed_within_and_bad_regex() {
        let dir = repo();
        let fixed = grep(dir.path(), None, &GrepOptions { fixed: true, ..opts("send(") }).unwrap();
        // The definition line and the one line in main.rs that calls it twice
        assert_eq!(fixed.hits, 2);

        let within = grep(dir.path(), None, &GrepOptions { within: Some("./src/net/".into()), ..opts("send") }).unwrap();
        assert!(within.groups.iter().all(|g| g.file.starts_with("src/net/")));
        assert_eq!(within.files_searched, 1);

        let err = grep(dir.path(), None, &opts("send(")).unwrap_err().to_string();
        assert!(err.contains("--fixed"), "{err}");
    }
}

//! `knot map`: a short orientation for a repository nobody has read yet. Directories
//! are clustered by where the code actually lives, each with its most called symbols,
//! plus the repo's most central non-test code by PageRank.

use anyhow::Result;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use super::load_corpus;
use crate::ast::resolve::list_supported_files;
use crate::ast::{compute_pagerank, CodeGraph, SupportedLang};

pub const DEFAULT_MAX_DIRS: usize = 12;
const HUBS_PER_DIR: usize = 3;
const HOTSPOTS: usize = 8;
/// A cluster holding more than this share of all files is split one level down.
const SPLIT_SHARE: f64 = 0.4;

#[derive(Debug, Clone, Serialize)]
pub struct Hub {
    pub name: String,
    pub kind: String,
    pub file: String,
    pub span: (usize, usize),
    pub callers: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct DirSummary {
    /// Directory prefix such as `src/requests/`, `(root)` for top level files.
    pub dir: String,
    pub files: usize,
    pub symbols: usize,
    pub hubs: Vec<Hub>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepoMap {
    pub files: usize,
    pub symbols: usize,
    /// Call edges plus import edges.
    pub edges: usize,
    pub languages: Vec<(String, usize)>,
    pub dirs: Vec<DirSummary>,
    /// Directories left out to stay within `max_dirs`.
    pub dropped_dirs: usize,
    pub hotspots: Vec<Hub>,
    pub learnings: usize,
}

fn dir_segments(file: &str) -> Vec<&str> {
    let mut parts: Vec<&str> = file.split('/').collect();
    parts.pop();
    parts
}

/// Longest directory prefix shared by every file, as `a/b/`, or "" when none.
fn common_dir(files: &[String]) -> String {
    let mut common: Option<Vec<&str>> = None;
    for f in files {
        let segs = dir_segments(f);
        common = Some(match common {
            None => segs,
            Some(prev) => prev.iter().zip(&segs).take_while(|(a, b)| a == b).map(|(a, _)| *a).collect(),
        });
    }
    let segs = common.unwrap_or_default();
    if segs.is_empty() {
        String::new()
    } else {
        format!("{}/", segs.join("/"))
    }
}

/// Group files by top level directory, collapse wrapper directories (`src/` holding
/// only `src/requests/`), and split any cluster holding most of the repo one level down.
fn cluster(files: &[String]) -> Vec<(String, Vec<String>)> {
    let mut top: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for f in files {
        let key = f.split_once('/').map_or(String::new(), |(d, _)| d.to_string());
        top.entry(key).or_default().push(f.clone());
    }

    let mut clusters = Vec::new();
    for (key, members) in top {
        if key.is_empty() {
            clusters.push(("(root)".to_string(), members));
            continue;
        }
        let prefix = common_dir(&members);
        let share = members.len() as f64 / files.len().max(1) as f64;
        let mut children: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for f in &members {
            let rest = &f[prefix.len()..];
            let child = match rest.split_once('/') {
                Some((d, _)) => format!("{prefix}{d}/"),
                None => prefix.clone(),
            };
            children.entry(child).or_default().push(f.clone());
        }
        if share > SPLIT_SHARE && children.len() >= 2 {
            clusters.extend(children);
        } else {
            clusters.push((prefix, members));
        }
    }
    clusters
}

fn hub(graph: &CodeGraph, id: &str) -> Option<Hub> {
    let s = graph.symbols.get(id)?;
    Some(Hub {
        name: s.name.clone(),
        kind: s.kind.as_str().to_string(),
        file: s.file.clone(),
        span: s.span,
        callers: graph.callers.get(id).map_or(0, Vec::len),
    })
}

fn count_learnings(root: &Path) -> usize {
    std::fs::read_dir(crate::engine::learnings::learnings_dir(root))
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
                .count()
        })
        .unwrap_or(0)
}

pub fn map(root: &Path, cache_root: Option<&Path>, max_dirs: usize) -> Result<RepoMap> {
    let (corpus, _) = load_corpus(root, cache_root)?;
    let graph = &corpus.graph;
    let tests = corpus.index.test_ids();
    let rank = compute_pagerank(graph);
    let score = |id: &str| rank.get(id).copied().unwrap_or(0.0);

    let files: Vec<String> = list_supported_files(root)?
        .iter()
        .map(|p| p.strip_prefix(root).unwrap_or(p).to_string_lossy().replace('\\', "/"))
        .collect();

    let mut languages: HashMap<&str, usize> = HashMap::new();
    for f in &files {
        if let Some(lang) = SupportedLang::from_path(Path::new(f)) {
            *languages.entry(lang.name()).or_default() += 1;
        }
    }
    let mut languages: Vec<(String, usize)> = languages.into_iter().map(|(l, n)| (l.to_string(), n)).collect();
    languages.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let mut dirs: Vec<DirSummary> = cluster(&files)
        .into_iter()
        .map(|(dir, members)| {
            let member_set: HashSet<&str> = members.iter().map(String::as_str).collect();
            let ids: Vec<&String> = members.iter().filter_map(|f| graph.file_to_ids.get(f)).flatten().collect();
            let mut candidates: Vec<&String> = ids
                .iter()
                .copied()
                .filter(|id| !tests.contains(id.as_str()))
                .filter(|id| graph.symbols.get(*id).is_some_and(|s| member_set.contains(s.file.as_str())))
                .collect();
            candidates.sort_by(|a, b| {
                let callers = |id: &str| graph.callers.get(id).map_or(0, Vec::len);
                callers(b).cmp(&callers(a)).then(score(b).total_cmp(&score(a))).then_with(|| a.cmp(b))
            });
            let hubs = candidates
                .into_iter()
                .filter_map(|id| hub(graph, id))
                .filter(|h| h.callers > 0)
                .take(HUBS_PER_DIR)
                .collect();
            DirSummary { dir, files: members.len(), symbols: ids.len(), hubs }
        })
        .collect();
    dirs.sort_by(|a, b| b.symbols.cmp(&a.symbols).then(b.files.cmp(&a.files)).then_with(|| a.dir.cmp(&b.dir)));
    let dropped_dirs = dirs.len().saturating_sub(max_dirs);
    dirs.truncate(max_dirs);

    let mut central: Vec<&String> = graph.symbols.keys().filter(|id| !tests.contains(id.as_str())).collect();
    central.sort_by(|a, b| score(b).total_cmp(&score(a)).then_with(|| a.cmp(b)));
    let hotspots = central.into_iter().filter_map(|id| hub(graph, id)).take(HOTSPOTS).collect();

    Ok(RepoMap {
        files: files.len(),
        symbols: graph.symbols.len(),
        edges: graph.callees.values().map(Vec::len).sum::<usize>() + graph.importers.values().map(Vec::len).sum::<usize>(),
        languages,
        dirs,
        dropped_dirs,
        hotspots,
        learnings: count_learnings(root),
    })
}

fn short_file(file: &str) -> &str {
    file.rsplit('/').next().unwrap_or(file)
}

pub fn format_map(map: &RepoMap) -> String {
    let langs: Vec<String> = map.languages.iter().map(|(l, n)| format!("{l} {n}")).collect();
    let mut out = format!(
        "repo map: {} files · {} symbols · {} edges · {}\n\n",
        map.files,
        map.symbols,
        map.edges,
        langs.join(", ")
    );

    let width = map.dirs.iter().map(|d| d.dir.len()).max().unwrap_or(0).max(8);
    for d in &map.dirs {
        let mut line = format!("{:<width$}  {:>4} files · {:>5} symbols", d.dir, d.files, d.symbols);
        if !d.hubs.is_empty() {
            let hubs: Vec<String> =
                d.hubs.iter().map(|h| format!("{} ({}, {}←)", h.name, short_file(&h.file), h.callers)).collect();
            line.push_str(&format!("   hubs: {}", hubs.join(", ")));
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    if map.dropped_dirs > 0 {
        out.push_str(&format!("(+{} smaller directories)\n", map.dropped_dirs));
    }

    if !map.hotspots.is_empty() {
        out.push_str("\nmost central code (PageRank, tests excluded):\n");
        for h in &map.hotspots {
            out.push_str(&format!(
                "  {} · {} · {}:L{}-L{} · {} caller(s)\n",
                h.name, h.kind, h.file, h.span.0, h.span.1, h.callers
            ));
        }
    }
    if map.learnings > 0 {
        out.push_str(&format!(
            "\n{} repo learning(s) in .knot/learnings; knot ask shows the related ones with code.\n",
            map.learnings
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strs(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn wrappers_collapse_and_dominant_dirs_split() {
        let files = strs(&[
            "setup.py",
            "src/requests/api.py",
            "src/requests/models.py",
            "tests/test_api.py",
            "app/ui/a.ts",
            "app/ui/b.ts",
            "app/ui/c.ts",
            "app/server/d.ts",
            "app/server/e.ts",
            "app/main.ts",
        ]);
        let clusters: Vec<String> = cluster(&files).into_iter().map(|(dir, _)| dir).collect();
        assert_eq!(
            clusters,
            vec!["(root)", "app/", "app/server/", "app/ui/", "src/requests/", "tests/"],
            "app holds 60% so it splits; src/ wraps only src/requests/"
        );
    }

    #[test]
    fn map_reports_dirs_hubs_and_central_non_test_code() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("tests")).unwrap();
        std::fs::write(root.join("src/core.rs"), "pub fn parse() {}\npub fn run() { parse(); }\n").unwrap();
        std::fs::write(root.join("src/cli.rs"), "pub fn main_entry() { run(); parse(); }\n").unwrap();
        std::fs::write(
            root.join("tests/it.rs"),
            "fn fixture() {}\n#[test]\nfn a() { fixture(); parse(); }\n#[test]\nfn b() { fixture(); }\n",
        )
        .unwrap();

        let m = map(root, None, DEFAULT_MAX_DIRS).unwrap();
        assert_eq!((m.files, m.symbols), (3, 6));
        assert_eq!(m.languages, vec![("Rust".to_string(), 3)]);

        let src = m.dirs.iter().find(|d| d.dir == "src/").unwrap();
        assert_eq!(src.hubs[0].name, "parse");
        assert_eq!(src.hubs[0].callers, 3);
        assert!(m.hotspots.iter().all(|h| !h.file.starts_with("tests/")), "tests stay out of hotspots");
        assert_eq!(m.hotspots[0].name, "parse");

        let text = format_map(&m);
        assert!(text.starts_with("repo map: 3 files · 6 symbols"), "{text}");
        assert!(text.contains("hubs: parse (core.rs, 3←)"), "{text}");
    }
}

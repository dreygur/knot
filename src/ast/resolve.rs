use anyhow::{Context, Result};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use super::extract::{extract_file, extract_file_skeleton, SupportedLang};
use super::types::{BlastRadiusReport, ImportRef, SkeletonItem, SymbolNode};

/// Build and vendor directories skipped even when no `.gitignore` lists them.
const ALWAYS_SKIPPED_DIRS: &[&str] = &[
    ".git", "target", "node_modules", "reference", ".knot", ".trail", ".idea", ".vscode", "dist", "build",
];

/// List supported source files under `root`, honoring `.gitignore`, `.ignore` and git
/// exclude files. Ignore files above `root` are not consulted, so indexing an ignored
/// directory directly (e.g. a vendored reference checkout) still works.
fn list_supported_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let walker = ignore::WalkBuilder::new(root)
        .follow_links(false)
        .parents(false)
        .filter_entry(|e| {
            let is_dir = e.file_type().is_some_and(|t| t.is_dir());
            !(is_dir && e.depth() > 0 && ALWAYS_SKIPPED_DIRS.contains(&e.file_name().to_string_lossy().as_ref()))
        })
        .build();

    for entry in walker {
        let entry = entry?;
        if entry.file_type().is_some_and(|t| t.is_file()) && SupportedLang::from_path(entry.path()).is_some() {
            files.push(entry.into_path());
        }
    }

    Ok(files)
}

/// Cheap change detector for indexed sources: file count, total size, newest mtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceFingerprint {
    pub files: usize,
    pub total_bytes: u64,
    pub newest_mtime_ns: u128,
}

/// Stat every supported source file without parsing, so callers can skip rebuilding
/// the graph when nothing changed.
pub fn source_fingerprint(root: &Path) -> Result<SourceFingerprint> {
    let mut fp = SourceFingerprint {
        files: 0,
        total_bytes: 0,
        newest_mtime_ns: 0,
    };
    for path in list_supported_files(root)? {
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        fp.files += 1;
        fp.total_bytes += meta.len();
        if let Some(ns) = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos())
        {
            fp.newest_mtime_ns = fp.newest_mtime_ns.max(ns);
        }
    }
    Ok(fp)
}

#[derive(Debug, Clone, Default)]
pub struct CodeGraph {
    pub symbols: HashMap<String, SymbolNode>,
    pub name_to_ids: HashMap<String, Vec<String>>,
    pub file_to_ids: HashMap<String, Vec<String>>,
    pub callers: HashMap<String, Vec<String>>,
    pub callees: HashMap<String, Vec<String>>,
    /// Import edges: symbol id -> files that import it.
    pub importers: HashMap<String, Vec<String>>,
}

/// A bare call name matching more symbols than this, with no import, qualifier or
/// same-file match to narrow it, is left unresolved rather than linked to all of them.
const MAX_UNRESOLVED_CANDIDATES: usize = 3;

/// Path segments that carry no location information.
const NON_LOCATING_SEGMENTS: &[&str] = &["crate", "self", "super", "Self", "this", "src", "lib", ""];

/// Turn an import module (`crate::engine::hybrid`, `pkg.billing`, `../fmt`) into a
/// repo-relative path suffix (`engine/hybrid`) that a defining file should end with.
fn module_path_hint(module: &str, importer_file: &str) -> Option<String> {
    if (module.starts_with('.') && module.contains('/')) || module == "." || module == ".." {
        // Relative JS/TS import: resolve against the importing file's directory. The
        // result is already a full repo-relative path, so keep every segment.
        let mut parts: Vec<&str> = importer_file.split('/').collect();
        parts.pop();
        for seg in module.split('/') {
            match seg {
                "." | "" => {}
                ".." => {
                    parts.pop();
                }
                s => parts.push(s),
            }
        }
        return (!parts.is_empty()).then(|| parts.join("/"));
    }

    let path = module.replace("::", "/").replace('.', "/");
    let segments: Vec<&str> = path.split('/').filter(|s| !NON_LOCATING_SEGMENTS.contains(s)).collect();
    (!segments.is_empty()).then(|| segments.join("/"))
}

/// Whether `file` (repo-relative) is a plausible definition site for `hint`.
fn file_matches_hint(file: &str, hint: &str) -> bool {
    let stem = file.rsplit_once('.').map(|(s, _)| s).unwrap_or(file);
    let stem = ["/mod", "/index", "/__init__"]
        .iter()
        .find_map(|suffix| stem.strip_suffix(suffix))
        .unwrap_or(stem);
    stem == hint || stem.ends_with(&format!("/{hint}"))
}

impl CodeGraph {
    /// Resolve one outgoing call from `caller_file` to target symbol ids.
    ///
    /// Order: an exact qualified name (`Type::new`), then same-file definitions, then
    /// definitions in the module the name was imported from (or the call's module
    /// qualifier), then — only when few — every symbol with that name.
    fn resolve_call(&self, caller_file: &str, call: &str, imports: &[ImportRef]) -> Vec<String> {
        let segments: Vec<&str> = call.split(['.', ':']).filter(|s| !s.is_empty()).collect();
        let Some((&bare, qualifier)) = segments.split_last() else {
            return Vec::new();
        };

        if let Some(&owner) = qualifier.last() {
            for sep in ["::", "."] {
                if let Some(ids) = self.name_to_ids.get(&format!("{owner}{sep}{bare}")) {
                    return ids.clone();
                }
            }
        }

        let Some(candidates) = self.name_to_ids.get(bare) else {
            return Vec::new();
        };

        let same_file: Vec<String> = candidates
            .iter()
            .filter(|id| self.symbols.get(*id).is_some_and(|s| s.file == caller_file))
            .cloned()
            .collect();
        if !same_file.is_empty() && qualifier.is_empty() {
            return same_file;
        }

        // Module hints: where `bare` (or the qualifier's first segment) was imported from,
        // plus the qualifier itself as a module path (`hybrid::resolve`).
        // A qualified call (`gamma::flush`) is located by its qualifier only; an import
        // of the bare name (`use beta::flush`) refers to a different item.
        let mut hints = Vec::new();
        let head = qualifier.first().copied().unwrap_or(bare);
        for imp in imports.iter().filter(|i| i.local == head) {
            let module = if !qualifier.is_empty() && imp.name != "*" {
                format!("{}::{}", imp.module, imp.name)
            } else {
                imp.module.clone()
            };
            hints.extend(module_path_hint(&module, caller_file));
        }
        if !qualifier.is_empty() {
            hints.extend(module_path_hint(&qualifier.join("::"), caller_file));
        }

        for hint in &hints {
            let matched: Vec<String> = candidates
                .iter()
                .filter(|id| self.symbols.get(*id).is_some_and(|s| file_matches_hint(&s.file, hint)))
                .cloned()
                .collect();
            if !matched.is_empty() {
                return matched;
            }
        }

        if candidates.len() <= MAX_UNRESOLVED_CANDIDATES {
            candidates.clone()
        } else {
            Vec::new()
        }
    }

    /// Files that import the given symbol (import edges), sorted.
    pub fn get_importers(&self, symbol_query: &str) -> Vec<String> {
        let mut files: Vec<String> = self
            .resolve_symbol(symbol_query)
            .iter()
            .filter_map(|id| self.importers.get(id))
            .flatten()
            .cloned()
            .collect();
        files.sort();
        files.dedup();
        files
    }

    pub fn build(repo_root: &Path) -> Result<Self> {
        let files = list_supported_files(repo_root)?;

        type ParsedFile = (String, Vec<SymbolNode>, Vec<ImportRef>);
        let extracted_results: Vec<Result<ParsedFile>> = files
            .par_iter()
            .map(|abs_path| {
                let rel_path = abs_path
                    .strip_prefix(repo_root)
                    .unwrap_or(abs_path)
                    .to_string_lossy()
                    .replace('\\', "/");

                let lang = SupportedLang::from_path(abs_path)
                    .context("Unsupported language for file")?;
                let content = std::fs::read_to_string(abs_path)?;
                let (file_symbols, imports) = extract_file(&rel_path, &content, lang)?;
                Ok((rel_path, file_symbols, imports))
            })
            .collect();

        let mut symbols = HashMap::new();
        let mut name_to_ids: HashMap<String, Vec<String>> = HashMap::new();
        let mut file_to_ids: HashMap<String, Vec<String>> = HashMap::new();
        let mut file_imports: HashMap<String, Vec<ImportRef>> = HashMap::new();

        for res in extracted_results {
            if let Ok((rel_path, file_symbols, imports)) = res {
                file_imports.insert(rel_path, imports);
                for sym in file_symbols {
                    let id = sym.id.clone();
                    let file = sym.file.clone();
                    let name = sym.name.clone();

                    let bare_name = name
                        .split("::")
                        .last()
                        .unwrap_or(&name)
                        .split('.')
                        .last()
                        .unwrap_or(&name)
                        .to_string();

                    name_to_ids.entry(bare_name.clone()).or_default().push(id.clone());
                    if bare_name != name {
                        name_to_ids.entry(name).or_default().push(id.clone());
                    }

                    file_to_ids.entry(file).or_default().push(id.clone());
                    symbols.insert(id, sym);
                }
            }
        }

        let mut graph = Self {
            symbols,
            name_to_ids,
            file_to_ids,
            ..Default::default()
        };

        let no_imports = Vec::new();
        let mut callers: HashMap<String, Vec<String>> = HashMap::new();
        let mut callees: HashMap<String, Vec<String>> = HashMap::new();

        for (caller_id, caller_node) in &graph.symbols {
            let imports = file_imports.get(&caller_node.file).unwrap_or(&no_imports);
            for call in &caller_node.outgoing_calls {
                for target_id in graph.resolve_call(&caller_node.file, call, imports) {
                    if &target_id != caller_id {
                        callees.entry(caller_id.clone()).or_default().push(target_id.clone());
                        callers.entry(target_id).or_default().push(caller_id.clone());
                    }
                }
            }
        }

        let mut importers: HashMap<String, Vec<String>> = HashMap::new();
        for (file, imports) in &file_imports {
            for imp in imports.iter().filter(|i| i.name != "*" && i.name != "default") {
                let Some(hint) = module_path_hint(&imp.module, file) else { continue };
                let Some(candidates) = graph.name_to_ids.get(&imp.name) else { continue };
                for id in candidates {
                    if graph.symbols.get(id).is_some_and(|s| file_matches_hint(&s.file, &hint)) {
                        importers.entry(id.clone()).or_default().push(file.clone());
                    }
                }
            }
        }

        for list in callers.values_mut().chain(callees.values_mut()).chain(importers.values_mut()) {
            list.sort();
            list.dedup();
        }

        graph.callers = callers;
        graph.callees = callees;
        graph.importers = importers;
        Ok(graph)
    }

    pub fn resolve_symbol(&self, query: &str) -> Vec<String> {
        if let Some(sym) = self.symbols.get(query) {
            return vec![sym.id.clone()];
        }
        if let Some(ids) = self.name_to_ids.get(query) {
            return ids.clone();
        }

        let mut matches = Vec::new();
        for (id, _) in &self.symbols {
            if id.ends_with(query) || id.contains(query) {
                matches.push(id.clone());
            }
        }
        matches
    }

    pub fn get_callers(&self, symbol_query: &str) -> Vec<SymbolNode> {
        let target_ids = self.resolve_symbol(symbol_query);
        let mut caller_ids = HashSet::new();

        for tid in target_ids {
            if let Some(clist) = self.callers.get(&tid) {
                for cid in clist {
                    caller_ids.insert(cid.clone());
                }
            }
        }

        caller_ids
            .into_iter()
            .filter_map(|cid| self.symbols.get(&cid).cloned())
            .collect()
    }

    pub fn get_callees(&self, symbol_query: &str) -> Vec<SymbolNode> {
        let source_ids = self.resolve_symbol(symbol_query);
        let mut callee_ids = HashSet::new();

        for sid in source_ids {
            if let Some(clist) = self.callees.get(&sid) {
                for cid in clist {
                    callee_ids.insert(cid.clone());
                }
            }
        }

        callee_ids
            .into_iter()
            .filter_map(|cid| self.symbols.get(&cid).cloned())
            .collect()
    }

    pub fn blast_radius(&self, symbol_query: &str, max_depth: Option<usize>) -> BlastRadiusReport {
        let target_ids = self.resolve_symbol(symbol_query);
        let depth_limit = max_depth.unwrap_or(10);

        let mut visited_ids = HashSet::new();
        let mut queue: VecDeque<(String, usize)> = VecDeque::new();

        for tid in &target_ids {
            visited_ids.insert(tid.clone());
            queue.push_back((tid.clone(), 0));
        }

        while let Some((curr_id, curr_depth)) = queue.pop_front() {
            if curr_depth >= depth_limit {
                continue;
            }

            if let Some(caller_ids) = self.callers.get(&curr_id) {
                for cid in caller_ids {
                    if visited_ids.insert(cid.clone()) {
                        queue.push_back((cid.clone(), curr_depth + 1));
                    }
                }
            }
        }

        for tid in &target_ids {
            visited_ids.remove(tid);
        }

        let mut impacted_symbols: Vec<SymbolNode> = visited_ids
            .into_iter()
            .filter_map(|id| self.symbols.get(&id).cloned())
            .collect();

        impacted_symbols.sort_by(|a, b| a.id.cmp(&b.id));

        let mut impacted_files: Vec<String> = impacted_symbols
            .iter()
            .map(|s| s.file.clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        impacted_files.sort();

        BlastRadiusReport {
            target: symbol_query.to_string(),
            depth: depth_limit,
            impacted_symbols,
            impacted_files,
        }
    }

    pub fn file_skeleton(&self, file_path: &str) -> Vec<SkeletonItem> {
        let norm_path = file_path.replace('\\', "/");
        if let Some(ids) = self.file_to_ids.get(&norm_path) {
            let mut symbols: Vec<SymbolNode> = ids
                .iter()
                .filter_map(|id| self.symbols.get(id).cloned())
                .collect();
            symbols.sort_by_key(|s| s.span.0);
            extract_file_skeleton(&symbols)
        } else {
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_code_graph_callers_and_callees() -> Result<()> {
        let dir = tempdir()?;
        let file1_path = dir.path().join("service.rs");
        let mut f1 = File::create(&file1_path)?;
        writeln!(
            f1,
            r#"
pub fn run_service() {{
    init_db();
    start_server();
}}
"#
        )?;

        let file2_path = dir.path().join("db.rs");
        let mut f2 = File::create(&file2_path)?;
        writeln!(
            f2,
            r#"
pub fn init_db() {{
    println!("db ready");
}}

pub fn start_server() {{
    println!("server ready");
}}
"#
        )?;

        let graph = CodeGraph::build(dir.path())?;

        let callers_of_init = graph.get_callers("init_db");
        assert!(callers_of_init.iter().any(|s| s.name == "run_service"));

        let callees_of_run = graph.get_callees("run_service");
        assert!(callees_of_run.iter().any(|s| s.name == "init_db"));
        assert!(callees_of_run.iter().any(|s| s.name == "start_server"));

        let blast = graph.blast_radius("init_db", Some(3));
        assert!(blast.impacted_symbols.iter().any(|s| s.name == "run_service"));
        assert!(blast.impacted_files.iter().any(|f| f.ends_with("service.rs")));

        Ok(())
    }

    #[test]
    fn test_imports_and_qualifiers_disambiguate_calls() -> Result<()> {
        let dir = tempdir()?;
        let root = dir.path();
        std::fs::create_dir_all(root.join("src/engine"))?;

        // Four modules each define `new` and `flush`, so the bare names are ambiguous.
        for name in ["alpha", "beta", "gamma", "delta"] {
            std::fs::write(
                root.join(format!("src/engine/{name}.rs")),
                format!("pub struct {name}T;\nimpl {name}T {{ pub fn new() -> Self {{ {name}T }} }}\npub fn flush() {{}}\n"),
            )?;
        }
        std::fs::write(
            root.join("src/main.rs"),
            "use crate::engine::beta::flush;\nuse crate::engine::gamma;\n\
             fn run() { flush(); gamma::flush(); deltaT::new(); unknown_thing(); }\n",
        )?;

        let graph = CodeGraph::build(root)?;
        let mut callees: Vec<String> = graph.get_callees("run").into_iter().map(|s| s.id).collect();
        callees.sort();
        assert_eq!(
            callees,
            vec![
                "src/engine/beta.rs::flush".to_string(),
                "src/engine/delta.rs::deltaT::new".to_string(),
                "src/engine/gamma.rs::flush".to_string(),
            ]
        );

        // A bare `new()` with four definitions and no hint must not link to all of them.
        std::fs::write(root.join("src/main.rs"), "fn run() { new(); }\n")?;
        let graph = CodeGraph::build(root)?;
        assert!(graph.get_callees("run").is_empty());

        // Import edges: main.rs imports beta's flush.
        std::fs::write(root.join("src/main.rs"), "use crate::engine::beta::flush;\nfn run() {}\n")?;
        let graph = CodeGraph::build(root)?;
        assert_eq!(graph.importers.get("src/engine/beta.rs::flush"), Some(&vec!["src/main.rs".to_string()]));
        assert!(!graph.importers.contains_key("src/engine/alpha.rs::flush"));

        Ok(())
    }

    #[test]
    fn test_relative_ts_import_resolves_to_file() -> Result<()> {
        let dir = tempdir()?;
        let root = dir.path();
        std::fs::create_dir_all(root.join("src/ui"))?;
        for f in ["src/api.ts", "src/ui/api.ts", "src/other/api.ts", "src/x/api.ts"] {
            std::fs::create_dir_all(root.join(f).parent().unwrap())?;
            std::fs::write(root.join(f), "export function save() {}\n")?;
        }
        std::fs::write(root.join("src/ui/form.ts"), "import { save } from '../api';\nfunction submit() { save(); }\n")?;

        let graph = CodeGraph::build(root)?;
        let callees: Vec<String> = graph.get_callees("submit").into_iter().map(|s| s.id).collect();
        assert_eq!(callees, vec!["src/api.ts::save".to_string()]);
        Ok(())
    }

    #[test]
    fn test_graph_build_honors_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git2::Repository::init(root).unwrap();
        std::fs::write(root.join(".gitignore"), "generated/\n").unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("generated")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn kept() {}\n").unwrap();
        std::fs::write(root.join("generated/out.rs"), "pub fn dropped() {}\n").unwrap();

        let graph = CodeGraph::build(root).unwrap();
        assert!(graph.name_to_ids.contains_key("kept"));
        assert!(!graph.name_to_ids.contains_key("dropped"));
    }
}

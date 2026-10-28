use anyhow::{Context, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use super::extract::{extract_file, extract_file_skeleton, SupportedLang};
use super::tsconfig::TsAliases;
use super::types::{BlastRadiusReport, ImportRef, SkeletonItem, SymbolNode};

/// Build and vendor directories skipped even when no `.gitignore` lists them.
pub(crate) const ALWAYS_SKIPPED_DIRS: &[&str] = &[
    ".git", "target", "node_modules", "reference", ".knot", ".trail", ".idea", ".vscode", "dist", "build",
];

/// Bundled third party builds, skipped wherever they sit: their symbols are minifier
/// names that would crowd out the repo's own code.
const ALWAYS_SKIPPED_SUFFIXES: &[&str] = &[".min.js"];

/// Whether the index leaves out a file by its name alone, wherever it sits. The shell
/// swap asks too, so it never answers a grep that would have matched in such a file.
pub(crate) fn is_skipped_file(name: &str) -> bool {
    ALWAYS_SKIPPED_SUFFIXES.iter().any(|s| name.ends_with(s))
}

/// List supported source files under `root`, honoring `.gitignore`, `.ignore` and git
/// exclude files. Ignore files above `root` are not consulted, so indexing an ignored
/// directory directly (e.g. a vendored reference checkout) still works.
pub fn list_supported_files(root: &Path) -> Result<Vec<PathBuf>> {
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
        // `._name` files are macOS metadata written on non-Apple volumes, not source
        let name = entry.file_name().to_string_lossy();
        let apple_double = name.starts_with("._");
        if !apple_double
            && !is_skipped_file(&name)
            && entry.file_type().is_some_and(|t| t.is_file())
            && SupportedLang::from_path(entry.path()).is_some()
        {
            files.push(entry.into_path());
        }
    }

    Ok(files)
}

/// One source file's extraction result, the unit the ask cache stores.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedFile {
    pub rel_path: String,
    pub symbols: Vec<SymbolNode>,
    pub imports: Vec<ImportRef>,
}

/// Parse one file under `repo_root` into symbols and imports.
pub fn parse_file(repo_root: &Path, abs_path: &Path) -> Result<ParsedFile> {
    let rel_path = abs_path
        .strip_prefix(repo_root)
        .unwrap_or(abs_path)
        .to_string_lossy()
        .replace('\\', "/");
    let lang = SupportedLang::from_path(abs_path).context("Unsupported language for file")?;
    let content = std::fs::read_to_string(abs_path)?;
    let (symbols, imports) = extract_file(&rel_path, &content, lang)?;
    Ok(ParsedFile { rel_path, symbols, imports })
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
    /// tsconfig/jsconfig path aliases used to locate non-relative TS/JS imports.
    pub ts_aliases: TsAliases,
}

/// A bare call name matching more symbols than this, with no import, qualifier or
/// same-file match to narrow it, is left unresolved rather than linked to all of them.
const MAX_UNRESOLVED_CANDIDATES: usize = 3;

/// Method names every standard library defines on many types. A call like `x.is_empty()`
/// whose receiver type is unknown is far more likely the std method than a repo method
/// that happens to share the name, so it is not guessed.
const UBIQUITOUS_METHODS: &[&str] = &[
    "append", "clear", "clone", "contains", "extend", "filter", "find", "get", "insert", "is_empty", "items",
    "iter", "join", "keys", "len", "map", "next", "pop", "push", "read", "remove", "split", "to_string",
    "trim", "update", "values", "write",
    // Ruby and PHP conventions every object answers
    "each", "inspect", "merge", "size", "to_a", "to_h", "to_i", "to_s", "to_sym", "toArray", "__toString",
];

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
    if stem == hint || stem.ends_with(&format!("/{hint}")) {
        return true;
    }
    // PSR-4 autoloading maps `App\Models\User` onto `app/Models/User.php`, so PHP
    // namespaces and directories may differ in case
    if file.ends_with(".php") {
        let (stem, hint) = (stem.to_lowercase(), hint.to_lowercase());
        if stem == hint || stem.ends_with(&format!("/{hint}")) {
            return true;
        }
    }
    // A Go import names a package directory by its full module path
    // (`github.com/acme/app/internal/store`), which ends with the file's directory.
    match file.strip_suffix(".go").and_then(|f| f.rsplit_once('/')) {
        Some((dir, _)) => hint == dir || hint.ends_with(&format!("/{dir}")),
        None => false,
    }
}

fn dir_of(file: &str) -> &str {
    file.rsplit_once('/').map_or("", |(dir, _)| dir)
}

fn is_ts_or_js(file: &str) -> bool {
    matches!(
        SupportedLang::from_path(Path::new(file)),
        Some(SupportedLang::TypeScript | SupportedLang::Tsx)
    )
}

impl CodeGraph {
    /// Where an import of `module` from `importer` may be defined. A non-relative
    /// TS/JS import goes through the tsconfig aliases first; anything they do not map
    /// falls back to reading the module name as a path.
    fn module_hints(&self, module: &str, importer: &str) -> Vec<String> {
        if !module.starts_with('.') && is_ts_or_js(importer) {
            let aliased = self.ts_aliases.resolve(module, importer);
            if !aliased.is_empty() {
                return aliased;
            }
        }
        module_path_hint(module, importer).into_iter().collect()
    }

    /// Several types share the name `owner` (`Reader::open` in four directories): keep
    /// the ones in the file `owner` was imported from, or all of them when no import
    /// says which.
    fn narrow_by_import(&self, ids: &[String], owner: &str, caller_file: &str, imports: &[ImportRef]) -> Vec<String> {
        if ids.len() < 2 {
            return ids.to_vec();
        }
        let hints: Vec<String> = imports
            .iter()
            .filter(|i| i.local == owner)
            .flat_map(|i| {
                let module = if i.name == "*" || is_ts_or_js(caller_file) {
                    i.module.clone()
                } else {
                    format!("{}::{}", i.module, i.name)
                };
                self.module_hints(&module, caller_file)
            })
            .collect();
        let narrowed: Vec<String> = ids
            .iter()
            .filter(|id| {
                self.symbols
                    .get(*id)
                    .is_some_and(|s| hints.iter().any(|h| file_matches_hint(&s.file, h)))
            })
            .cloned()
            .collect();
        if narrowed.is_empty() {
            ids.to_vec()
        } else {
            narrowed
        }
    }

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
                    return self.narrow_by_import(ids, owner, caller_file, imports);
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

        // A Go package spans its directory, so an unqualified call can be in any file there.
        if qualifier.is_empty() && caller_file.ends_with(".go") {
            let same_package: Vec<String> = candidates
                .iter()
                .filter(|id| {
                    self.symbols
                        .get(*id)
                        .is_some_and(|s| s.file.ends_with(".go") && dir_of(&s.file) == dir_of(caller_file))
                })
                .cloned()
                .collect();
            if !same_package.is_empty() {
                return same_package;
            }
        }

        // C: a function called from a file defined in the `.c` beside a header it includes
        if qualifier.is_empty() && matches!(SupportedLang::from_path(Path::new(caller_file)), Some(SupportedLang::C)) {
            let hints: Vec<String> = imports
                .iter()
                .filter(|i| i.name == "*")
                .flat_map(|i| self.module_hints(&i.module, caller_file))
                .collect();
            let included: Vec<String> = candidates
                .iter()
                .filter(|id| {
                    self.symbols
                        .get(*id)
                        .is_some_and(|s| hints.iter().any(|h| file_matches_hint(&s.file, h)))
                })
                .cloned()
                .collect();
            if !included.is_empty() {
                return included;
            }
        }

        // Module hints: where `bare` (or the qualifier's first segment) was imported from,
        // plus the qualifier itself as a module path (`hybrid::resolve`).
        // A qualified call (`gamma::flush`) is located by its qualifier only; an import
        // of the bare name (`use beta::flush`) refers to a different item.
        let mut hints = Vec::new();
        let head = qualifier.first().copied().unwrap_or(bare);
        for imp in imports.iter().filter(|i| i.local == head) {
            // A TS/JS import names a file, never a member path inside one
            if is_ts_or_js(caller_file) {
                hints.extend(self.module_hints(&imp.module, caller_file));
                continue;
            }
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

        if candidates.len() > MAX_UNRESOLVED_CANDIDATES {
            return Vec::new();
        }
        if qualifier.is_empty() {
            return candidates.clone();
        }
        // A qualified call that no type, import or module path located: `fs::write` and
        // `path.join` name things outside the repo, so only a method can be guessed, and
        // not one of the names every std type has.
        if UBIQUITOUS_METHODS.contains(&bare) {
            return Vec::new();
        }
        // `Options::from_str` names its type; the exact `Type::method` lookup above
        // already failed, so it is some other type's method, likely outside the repo
        if qualifier.last().is_some_and(|q| q.starts_with(char::is_uppercase)) {
            return Vec::new();
        }
        // `serde_json::from_str` is a path, and module paths were tried above; only a
        // call on a value (`x.method()`) can be a repo method reached through a variable
        if call.contains("::") {
            return Vec::new();
        }
        candidates
            .iter()
            .filter(|id| self.symbols.get(*id).is_some_and(|s| s.kind == crate::ast::SymbolKind::Method))
            .cloned()
            .collect()
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
        let parsed: Vec<ParsedFile> = files
            .par_iter()
            .filter_map(|abs_path| parse_file(repo_root, abs_path).ok())
            .collect();
        Ok(Self::from_parsed_with_aliases(parsed, TsAliases::load(repo_root)))
    }

    /// Assemble the graph from already parsed files, e.g. ones loaded from a cache.
    pub fn from_parsed(parsed: Vec<ParsedFile>) -> Self {
        Self::from_parsed_with_aliases(parsed, TsAliases::default())
    }

    /// [`from_parsed`](Self::from_parsed), locating aliased TS/JS imports through `ts_aliases`.
    pub fn from_parsed_with_aliases(parsed: Vec<ParsedFile>, ts_aliases: TsAliases) -> Self {
        let mut symbols = HashMap::new();
        let mut name_to_ids: HashMap<String, Vec<String>> = HashMap::new();
        let mut file_to_ids: HashMap<String, Vec<String>> = HashMap::new();
        let mut file_imports: HashMap<String, Vec<ImportRef>> = HashMap::new();

        for ParsedFile { rel_path, symbols: file_symbols, imports } in parsed {
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

        let mut graph = Self {
            symbols,
            name_to_ids,
            file_to_ids,
            ts_aliases,
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
                let hints = graph.module_hints(&imp.module, file);
                let Some(candidates) = graph.name_to_ids.get(&imp.name) else { continue };
                for id in candidates {
                    let defined_there = graph
                        .symbols
                        .get(id)
                        .is_some_and(|s| hints.iter().any(|hint| file_matches_hint(&s.file, hint)));
                    if defined_there {
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
        graph
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
    fn minified_scripts_are_not_indexed() -> Result<()> {
        let dir = tempdir()?;
        let root = dir.path();
        std::fs::write(root.join("app.js"), "function app() {}\n")?;
        std::fs::write(root.join("sigma.min.js"), "function t(e){return e}\n")?;

        let names: Vec<String> = list_supported_files(root)?
            .iter()
            .map(|p| p.file_name().unwrap_or_default().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["app.js".to_string()]);
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
    fn test_c_includes_and_php_use_pick_the_right_definition() -> Result<()> {
        let dir = tempdir()?;
        let root = dir.path();
        // Four definitions of each name, more than the unresolved fallback links
        for d in ["net", "disk", "pipe", "tty"] {
            std::fs::create_dir_all(root.join("src").join(d))?;
            std::fs::write(root.join(format!("src/{d}/io.c")), "int io_read(int fd) { return fd; }\n")?;
            std::fs::create_dir_all(root.join("app").join(d))?;
            std::fs::write(
                root.join(format!("app/{d}/Reader.php")),
                "<?php\nclass Reader { public static function open() {} }\n",
            )?;
        }
        std::fs::write(root.join("src/server.c"), "#include \"net/io.h\"\nint serve(void) { return io_read(3); }\n")?;
        std::fs::write(
            root.join("app/Server.php"),
            "<?php\nuse App\\Pipe\\Reader;\nfunction serve() { Reader::open(); }\n",
        )?;

        let graph = CodeGraph::build(root)?;
        let c: Vec<String> = graph.get_callees("serve").into_iter().map(|s| s.file).collect();
        assert!(c.contains(&"src/net/io.c".to_string()), "{c:?}");
        assert!(c.contains(&"app/pipe/Reader.php".to_string()), "PSR-4 case mismatch still resolves: {c:?}");
        assert_eq!(c.len(), 2, "{c:?}");
        Ok(())
    }

    #[test]
    fn test_unlocated_qualified_calls_are_not_guessed() -> Result<()> {
        let dir = tempdir()?;
        let root = dir.path();
        std::fs::create_dir_all(root.join("src"))?;
        std::fs::write(
            root.join("src/lib.rs"),
            "pub struct Aliases;\nimpl Aliases {\n    pub fn is_empty(&self) -> bool { true }\n    pub fn rebalance(&self) {}\n}\n\
             pub fn write() {}\n",
        )?;
        std::fs::write(
            root.join("src/run.rs"),
            "fn run(v: Vec<u8>, t: Tree) {\n    v.is_empty();\n    std::fs::write(\"a\", \"b\");\n    Other::rebalance();\n    other_mod::rebalance();\n    t.rebalance();\n}\n",
        )?;

        let graph = CodeGraph::build(root)?;
        let callees: Vec<String> = graph.get_callees("run").into_iter().map(|s| s.name).collect();
        // `t.rebalance()` may well be the repo's method; std names and module paths are not
        assert_eq!(callees, vec!["Aliases::rebalance".to_string()]);
        Ok(())
    }

    #[test]
    fn test_tsconfig_alias_imports_resolve_to_the_aliased_file() -> Result<()> {
        let dir = tempdir()?;
        let root = dir.path();
        std::fs::write(root.join("tsconfig.json"), r#"{ "compilerOptions": { "paths": { "@/*": ["./src/*"] } } }"#)?;
        // Four `getUser`s, more than the unresolved fallback links, so only the alias can pick one
        for f in ["src/lib/auth.ts", "src/other/a.ts", "src/other/b.ts", "src/other/c.ts"] {
            std::fs::create_dir_all(root.join(f).parent().unwrap())?;
            std::fs::write(root.join(f), "export function getUser() {}\n")?;
        }
        std::fs::create_dir_all(root.join("src/app"))?;
        std::fs::write(
            root.join("src/app/page.tsx"),
            "import { getUser } from '@/lib/auth';\nexport function Page() { getUser(); }\n",
        )?;

        let graph = CodeGraph::build(root)?;
        let callees: Vec<String> = graph.get_callees("Page").into_iter().map(|s| s.id).collect();
        assert_eq!(callees, vec!["src/lib/auth.ts::getUser".to_string()]);
        let importers = graph.importers.get("src/lib/auth.ts::getUser").cloned().unwrap_or_default();
        assert_eq!(importers, vec!["src/app/page.tsx".to_string()]);
        Ok(())
    }

    #[test]
    fn test_go_calls_resolve_by_package_directory() -> Result<()> {
        let dir = tempdir()?;
        let root = dir.path();
        // Four `Open`s, more than the unresolved fallback links, so only the
        // package rules can pick one.
        for pkg in ["internal/store", "internal/cache", "internal/queue", "internal/log"] {
            std::fs::create_dir_all(root.join(pkg))?;
            std::fs::write(root.join(pkg).join("open.go"), "package p\n\nfunc Open() {}\n")?;
        }
        std::fs::write(
            root.join("internal/store/run.go"),
            "package store\n\nimport \"github.com/acme/app/internal/cache\"\n\n\
             func Run() {\n\tOpen()\n\tcache.Open()\n}\n",
        )?;

        let graph = CodeGraph::build(root)?;
        let mut callees: Vec<String> = graph.get_callees("Run").into_iter().map(|s| s.id).collect();
        callees.sort();
        assert_eq!(
            callees,
            vec!["internal/cache/open.go::Open".to_string(), "internal/store/open.go::Open".to_string()]
        );
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

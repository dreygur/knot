use anyhow::{Context, Result};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use super::extract::{extract_file_skeleton, extract_file_symbols, SupportedLang};
use super::types::{BlastRadiusReport, SkeletonItem, SymbolNode};

fn is_ignored_dir(entry: &walkdir::DirEntry) -> bool {
    let name = entry.file_name().to_string_lossy();
    if entry.file_type().is_dir() {
        matches!(
            name.as_ref(),
            ".git" | "target" | "node_modules" | "reference" | ".knot" | ".trail" | ".idea" | ".vscode" | "dist" | "build"
        )
    } else {
        false
    }
}

fn list_supported_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let walker = WalkDir::new(root).follow_links(false).into_iter();

    for entry in walker.filter_entry(|e| !is_ignored_dir(e)) {
        let entry = entry?;
        if entry.file_type().is_file() {
            let path = entry.path();
            if SupportedLang::from_path(path).is_some() {
                files.push(path.to_path_buf());
            }
        }
    }

    Ok(files)
}

#[derive(Debug, Clone, Default)]
pub struct CodeGraph {
    pub symbols: HashMap<String, SymbolNode>,
    pub name_to_ids: HashMap<String, Vec<String>>,
    pub file_to_ids: HashMap<String, Vec<String>>,
    pub callers: HashMap<String, Vec<String>>,
    pub callees: HashMap<String, Vec<String>>,
}

impl CodeGraph {
    pub fn build(repo_root: &Path) -> Result<Self> {
        let files = list_supported_files(repo_root)?;

        let extracted_results: Vec<Result<Vec<SymbolNode>>> = files
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
                extract_file_symbols(&rel_path, &content, lang)
            })
            .collect();

        let mut symbols = HashMap::new();
        let mut name_to_ids: HashMap<String, Vec<String>> = HashMap::new();
        let mut file_to_ids: HashMap<String, Vec<String>> = HashMap::new();

        for res in extracted_results {
            if let Ok(file_symbols) = res {
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

        let mut callers: HashMap<String, Vec<String>> = HashMap::new();
        let mut callees: HashMap<String, Vec<String>> = HashMap::new();

        for (caller_id, caller_node) in &symbols {
            for call_name in &caller_node.outgoing_calls {
                if let Some(target_ids) = name_to_ids.get(call_name) {
                    for target_id in target_ids {
                        if target_id != caller_id {
                            callees.entry(caller_id.clone()).or_default().push(target_id.clone());
                            callers.entry(target_id.clone()).or_default().push(caller_id.clone());
                        }
                    }
                }
            }
        }

        for list in callers.values_mut() {
            list.sort();
            list.dedup();
        }
        for list in callees.values_mut() {
            list.sort();
            list.dedup();
        }

        Ok(Self {
            symbols,
            name_to_ids,
            file_to_ids,
            callers,
            callees,
        })
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
}

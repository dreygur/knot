//! Check the `file#symbol` targets a learning touches, so `knot note` refuses one that
//! names a symbol the file does not have, or one of several that share the name.

use crate::ast::extract::{extract_file_symbols, SupportedLang};
use std::path::Path;

/// What a symbol name resolves to in one file.
enum SymbolLookup {
    Found,
    Missing,
    /// Several symbols share the name; holds their qualified names.
    Ambiguous(Vec<String>),
}

/// Split `file#symbol` (or `file.rs::symbol`) into the file and the symbol.
pub fn parse_target(target: &str) -> (&str, Option<&str>) {
    if let Some((path, symbol)) = target.split_once('#') {
        (path.trim(), Some(symbol.trim()))
    } else if let Some((path, symbol)) = target.split_once("::") {
        if path.contains('/') || path.contains('\\') || SupportedLang::from_path(Path::new(path)).is_some() {
            (path.trim(), Some(symbol.trim()))
        } else {
            (target, None)
        }
    } else {
        (target, None)
    }
}

/// Resolve `symbol_name` in `path`. An exact qualified name (`Type::method`) wins;
/// otherwise a bare name must match exactly one symbol. Overloads share a name, so
/// several exact matches are ambiguous too; each is named by its id suffix
/// (`Type.method#2`).
fn lookup_symbol(path: &Path, symbol_name: &str) -> Option<SymbolLookup> {
    let content = std::fs::read_to_string(path).ok()?;
    let lang = SupportedLang::from_path(path)?;
    let file = path.to_string_lossy();
    let symbols = extract_file_symbols(&file, &content, lang).ok()?;
    let id_name = |id: &str| id.strip_prefix(&format!("{file}::")).unwrap_or(id).to_string();

    let exact: Vec<_> = symbols
        .iter()
        .filter(|s| s.name == symbol_name || id_name(&s.id) == symbol_name)
        .collect();
    match exact.as_slice() {
        [_] => return Some(SymbolLookup::Found),
        [] => {}
        many => return Some(SymbolLookup::Ambiguous(many.iter().map(|s| id_name(&s.id)).collect())),
    }

    let candidates: Vec<_> = symbols
        .iter()
        .filter(|s| s.name.ends_with(&format!("::{symbol_name}")) || s.name.ends_with(&format!(".{symbol_name}")))
        .collect();

    Some(match candidates.as_slice() {
        [] => SymbolLookup::Missing,
        [_] => SymbolLookup::Found,
        many => SymbolLookup::Ambiguous(many.iter().map(|s| s.name.clone()).collect()),
    })
}

/// Why a `file#symbol` target does not name one existing symbol, or `None` when it does.
pub fn target_error(target: &str) -> Option<String> {
    let (file_path, symbol) = parse_target(target);
    let path = Path::new(file_path);
    if !path.exists() {
        return Some(format!("path does not exist: {file_path}"));
    }
    match symbol.and_then(|sym| lookup_symbol(path, sym).map(|l| (sym, l))) {
        Some((sym, SymbolLookup::Missing)) => Some(format!("symbol '{sym}' not found in {file_path}")),
        Some((sym, SymbolLookup::Ambiguous(names))) => Some(format!(
            "symbol '{sym}' is ambiguous in {file_path}; qualify it as one of: {}",
            names.join(", ")
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn overloads_are_ambiguous_until_qualified_by_id() -> anyhow::Result<()> {
        let dir = tempdir()?;
        let file_path = dir.path().join("Billing.java");
        std::fs::write(
            &file_path,
            "class Billing {\n  void charge(String id) {}\n  void charge(String id, int cents) {}\n}\n",
        )?;
        let base = file_path.to_string_lossy().to_string();

        let err = target_error(&format!("{base}#Billing.charge")).unwrap();
        assert!(err.contains("Billing.charge, Billing.charge#2"), "{err}");
        assert_eq!(target_error(&format!("{base}#Billing.charge#2")), None);
        Ok(())
    }

    #[test]
    fn ambiguous_bare_symbol_requires_qualification() -> anyhow::Result<()> {
        let dir = tempdir()?;
        let file_path = dir.path().join("types.rs");
        std::fs::write(
            &file_path,
            "struct A;\nimpl A { fn new() -> Self { A } }\nstruct B;\nimpl B { fn new() -> Self { B } }\n",
        )?;
        let base = file_path.to_string_lossy().to_string();

        assert!(target_error(&format!("{base}#new")).unwrap().contains("A::new, B::new"));
        assert_eq!(target_error(&format!("{base}#B::new")), None);
        assert!(target_error(&format!("{base}#gone")).unwrap().contains("not found"));
        Ok(())
    }

    #[test]
    fn targets_split_on_hash_or_a_file_path() {
        assert_eq!(parse_target("src/a.rs#run"), ("src/a.rs", Some("run")));
        assert_eq!(parse_target("src/a.rs::run"), ("src/a.rs", Some("run")));
        assert_eq!(parse_target("crate::engine"), ("crate::engine", None));
    }
}

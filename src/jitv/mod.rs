use crate::ast::extract::{extract_file_symbols, SupportedLang};
use crate::memory::node::{KnowledgeNode, VerificationResult, VerificationStatus};
use std::path::Path;

pub fn parse_target(target: &str) -> (&str, Option<&str>) {
    if let Some((path, symbol)) = target.split_once('#') {
        (path.trim(), Some(symbol.trim()))
    } else if let Some((path, symbol)) = target.split_once("::") {
        if path.contains('/') || path.contains('\\') || path.ends_with(".rs") || path.ends_with(".py") || path.ends_with(".ts") {
            (path.trim(), Some(symbol.trim()))
        } else {
            (target, None)
        }
    } else {
        (target, None)
    }
}

pub fn hash_path(path: &str) -> Option<String> {
    crate::utils::calculate_hash(Path::new(path)).ok()
}

pub fn hash_target(target: &str) -> Option<String> {
    let (file_path, symbol_opt) = parse_target(target);
    let path = Path::new(file_path);

    if !path.exists() {
        return None;
    }

    if let Some(symbol_name) = symbol_opt {
        match lookup_symbol(path, symbol_name)? {
            SymbolLookup::Found(hash) => Some(hash),
            SymbolLookup::Missing | SymbolLookup::Ambiguous(_) => None,
        }
    } else {
        hash_path(file_path)
    }
}

/// Result of resolving the symbol half of a `file#symbol` target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymbolLookup {
    Found(String),
    Missing,
    /// Several symbols share the name; holds their qualified names.
    Ambiguous(Vec<String>),
}

/// Resolve `symbol_name` in `path`. An exact qualified name (`Type::method`) wins;
/// otherwise a bare name must match exactly one symbol, since hashing an arbitrary
/// one of several would verify the wrong code.
pub fn lookup_symbol(path: &Path, symbol_name: &str) -> Option<SymbolLookup> {
    let lang = SupportedLang::from_path(path)?;
    let content = std::fs::read_to_string(path).ok()?;
    let symbols = extract_file_symbols(&path.to_string_lossy(), &content, lang).ok()?;

    if let Some(exact) = symbols.iter().find(|s| s.name == symbol_name) {
        return Some(SymbolLookup::Found(exact.hash.clone()));
    }

    let candidates: Vec<_> = symbols
        .iter()
        .filter(|s| s.name.ends_with(&format!("::{symbol_name}")) || s.name.ends_with(&format!(".{symbol_name}")))
        .collect();

    Some(match candidates.as_slice() {
        [] => SymbolLookup::Missing,
        [only] => SymbolLookup::Found(only.hash.clone()),
        many => SymbolLookup::Ambiguous(many.iter().map(|s| s.name.clone()).collect()),
    })
}

/// Explain why a `file#symbol` target cannot be hashed, or `None` when it can.
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

pub fn annotate(node: &KnowledgeNode, result: &VerificationResult) -> Option<String> {
    let tag = result.status.tag();
    if tag.is_empty() {
        Some(node.content.clone())
    } else {
        Some(format!("{tag} {}", node.content))
    }
}

pub fn verify(node: &KnowledgeNode) -> VerificationResult {
    let Some(ref raw_target) = node.verification_path else {
        return VerificationResult {
            node_id: node.id,
            status: VerificationStatus::Abstract,
            detail: "No verification path: abstract knowledge".into(),
        };
    };

    let (file_path, symbol_opt) = parse_target(raw_target);
    if !Path::new(file_path).exists() {
        return VerificationResult {
            node_id: node.id,
            status: VerificationStatus::StaleMissing,
            detail: format!("Path no longer exists: {file_path}"),
        };
    }

    let current_hash = hash_target(raw_target);

    match (&node.content_hash, current_hash) {
        (Some(stored), Some(current)) if stored == &current => VerificationResult {
            node_id: node.id,
            status: VerificationStatus::Verified,
            detail: "Hash verified".into(),
        },
        (Some(stored), Some(current)) => VerificationResult {
            node_id: node.id,
            status: VerificationStatus::StaleModified,
            detail: format!(
                "Content changed: stored={}, current={}",
                &stored[..8.min(stored.len())],
                &current[..8.min(current.len())]
            ),
        },
        (None, _) => VerificationResult {
            node_id: node.id,
            status: VerificationStatus::Verified,
            detail: "No stored hash: path confirmed".into(),
        },
        (_, None) => {
            if let Some(sym) = symbol_opt {
                VerificationResult {
                    node_id: node.id,
                    status: VerificationStatus::StaleMissing,
                    detail: target_error(raw_target).unwrap_or_else(|| format!("Symbol '{sym}' not found in {file_path}")),
                }
            } else {
                VerificationResult {
                    node_id: node.id,
                    status: VerificationStatus::StaleMissing,
                    detail: format!("Could not read path: {file_path}"),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{KnowledgeNode, MemoryScope};
    use std::fs::File;
    use std::io::Write;
    use tempfile::tempdir;

    fn make_node(path: Option<String>, hash: Option<String>) -> KnowledgeNode {
        KnowledgeNode {
            id: uuid::Uuid::new_v4(),
            content: "test wisdom".into(),
            tags: vec![],
            verification_path: path,
            content_hash: hash,
            utility_score: 0.5,
            scope: MemoryScope::Global,
            is_stale: false,
            parent_id: None,
            origin_agent: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn abstract_node_always_verified() {
        let node = make_node(None, None);
        let result = verify(&node);
        assert_eq!(result.status, VerificationStatus::Abstract);
    }

    #[test]
    fn symbol_level_verification_survives_unrelated_edits() -> anyhow::Result<()> {
        let dir = tempdir()?;
        let file_path = dir.path().join("auth.rs");
        let path_str = file_path.to_string_lossy().to_string();

        let initial_code = r#"
pub fn helper() {
    println!("v1");
}

pub fn validate_jwt(token: &str) -> bool {
    token.starts_with("Bearer ")
}
"#;
        let mut f = File::create(&file_path)?;
        write!(f, "{initial_code}")?;

        let target = format!("{path_str}#validate_jwt");
        let initial_hash = hash_target(&target);
        assert!(initial_hash.is_some());

        let node = make_node(Some(target.clone()), initial_hash.clone());
        let res1 = verify(&node);
        assert_eq!(res1.status, VerificationStatus::Verified);

        // Edit unrelated function and add comments
        let edited_code = r#"
// Brand new comment
pub fn helper() {
    println!("v2 modified");
}

pub fn validate_jwt(token: &str) -> bool {
    token.starts_with("Bearer ")
}
"#;
        let mut f = File::create(&file_path)?;
        write!(f, "{edited_code}")?;

        // Symbol-level verify must STILL BE VERIFIED!
        let res2 = verify(&node);
        assert_eq!(res2.status, VerificationStatus::Verified);

        // Now modify the actual validate_jwt symbol
        let changed_symbol_code = r#"
pub fn validate_jwt(token: &str) -> bool {
    token.len() > 10
}
"#;
        let mut f = File::create(&file_path)?;
        write!(f, "{changed_symbol_code}")?;

        let res3 = verify(&node);
        assert_eq!(res3.status, VerificationStatus::StaleModified);

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

        assert!(hash_target(&format!("{base}#new")).is_none());
        assert!(target_error(&format!("{base}#new")).unwrap().contains("A::new, B::new"));

        let b_target = format!("{base}#B::new");
        let node = make_node(Some(b_target.clone()), hash_target(&b_target));
        assert!(node.content_hash.is_some());
        assert_eq!(verify(&node).status, VerificationStatus::Verified);

        std::fs::write(
            &file_path,
            "struct A;\nimpl A { fn new() -> Self { A } }\nstruct B;\nimpl B { fn new() -> Self { B {} } }\n",
        )?;
        assert_eq!(verify(&node).status, VerificationStatus::StaleModified);
        Ok(())
    }
}

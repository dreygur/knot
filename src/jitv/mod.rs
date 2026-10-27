use crate::ast::extract::{extract_file_symbols, SupportedLang};
use crate::memory::node::{KnowledgeNode, VerificationResult, VerificationStatus};
use crate::memory::MemoryScope;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

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

pub fn hash_path(path: &str) -> Option<String> {
    crate::utils::calculate_hash(Path::new(path)).ok()
}

/// The real path of an open file, read from its descriptor rather than its name.
#[cfg(target_os = "macos")]
fn opened_path(file: &File) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::io::AsRawFd;
    let mut buf = vec![0u8; libc::PATH_MAX as usize];
    // SAFETY: F_GETPATH writes at most PATH_MAX bytes, NUL terminated, into `buf`.
    let rc = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETPATH, buf.as_mut_ptr()) };
    if rc == -1 {
        return None;
    }
    let len = buf.iter().position(|&b| b == 0)?;
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(&buf[..len])))
}

#[cfg(target_os = "linux")]
fn opened_path(file: &File) -> Option<PathBuf> {
    use std::os::unix::io::AsRawFd;
    std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd())).ok()
}

/// Elsewhere the name is resolved separately from the open, which narrows the race
/// with a swapped symlink but cannot close it.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn opened_path(_file: &File) -> Option<PathBuf> {
    None
}

/// Read the file behind `path`. With a `root`, the file actually opened must lie inside
/// it: the check is made on the open handle and the bytes come from that same handle, so
/// swapping a path component for a symlink after an earlier check cannot redirect the read.
pub fn read_target_file(path: &Path, root: Option<&Path>) -> Option<Vec<u8>> {
    let mut file = File::open(path).ok()?;
    if let Some(root) = root {
        let root = std::fs::canonicalize(root).ok()?;
        let opened = opened_path(&file).or_else(|| std::fs::canonicalize(path).ok())?;
        if !opened.starts_with(&root) {
            return None;
        }
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

/// The directory a memory's target must stay inside: the repository of a learning
/// synced from `.knot/learnings`, whose project id is the repo's canonical path.
/// Memories the user saved themselves name any file they like and have no root.
pub fn containment_root(node: &KnowledgeNode) -> Option<PathBuf> {
    match &node.scope {
        MemoryScope::Project(id) if Path::new(id).is_absolute() => Some(PathBuf::from(id)),
        _ => None,
    }
}

pub fn hash_target(target: &str) -> Option<String> {
    hash_target_within(target, None)
}

/// [`hash_target`], refusing a file that is not inside `root` when read.
pub fn hash_target_within(target: &str, root: Option<&Path>) -> Option<String> {
    let (file_path, symbol_opt) = parse_target(target);
    let path = Path::new(file_path);
    let bytes = read_target_file(path, root)?;

    match symbol_opt {
        Some(symbol_name) => match lookup_symbol_in(path, &String::from_utf8(bytes).ok()?, symbol_name)? {
            SymbolLookup::Found(hash) => Some(hash),
            SymbolLookup::Missing | SymbolLookup::Ambiguous(_) => None,
        },
        None => Some(blake3::hash(&bytes).to_hex().to_string()),
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
    let content = std::fs::read_to_string(path).ok()?;
    lookup_symbol_in(path, &content, symbol_name)
}

/// [`lookup_symbol`] over source already read. Overloads share a name, so several exact
/// matches are ambiguous too; each is named by its id suffix (`Type.method#2`).
fn lookup_symbol_in(path: &Path, content: &str, symbol_name: &str) -> Option<SymbolLookup> {
    let lang = SupportedLang::from_path(path)?;
    let file = path.to_string_lossy();
    let symbols = extract_file_symbols(&file, content, lang).ok()?;
    let id_name = |id: &str| id.strip_prefix(&format!("{file}::")).unwrap_or(id).to_string();

    let exact: Vec<_> = symbols
        .iter()
        .filter(|s| s.name == symbol_name || id_name(&s.id) == symbol_name)
        .collect();
    match exact.as_slice() {
        [only] => return Some(SymbolLookup::Found(only.hash.clone())),
        [] => {}
        many => return Some(SymbolLookup::Ambiguous(many.iter().map(|s| id_name(&s.id)).collect())),
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

    // A learning's file must still be inside its repo when read, not only when synced
    let root = containment_root(node);
    if let Some(root) = root.as_deref() {
        if read_target_file(Path::new(file_path), Some(root)).is_none() {
            return VerificationResult {
                node_id: node.id,
                status: VerificationStatus::StaleMissing,
                detail: format!("Path no longer resolves inside the repository: {file_path}"),
            };
        }
    }

    let current_hash = hash_target_within(raw_target, root.as_deref());

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

    #[cfg(unix)]
    #[test]
    fn learning_target_swapped_for_symlink_out_of_repo_is_refused() -> anyhow::Result<()> {
        let repo = tempdir()?;
        let outside = tempdir()?;
        let repo_root = std::fs::canonicalize(repo.path())?;
        std::fs::create_dir_all(repo_root.join("src"))?;
        std::fs::write(repo_root.join("src/db.rs"), "pub fn open_pool() {}\n")?;
        std::fs::write(outside.path().join("db.rs"), "pub fn open_pool() {}\n")?;

        let target = format!("{}#open_pool", repo_root.join("src/db.rs").display());
        let mut node = make_node(Some(target), None);
        node.scope = MemoryScope::Project(repo_root.to_string_lossy().into_owned());
        assert_eq!(verify(&node).status, VerificationStatus::Verified);

        // After the sync-time check, `src` becomes a link to a directory outside the repo
        std::fs::remove_dir_all(repo_root.join("src"))?;
        std::os::unix::fs::symlink(outside.path(), repo_root.join("src"))?;
        let result = verify(&node);
        assert_eq!(result.status, VerificationStatus::StaleMissing);
        assert!(result.detail.contains("inside the repository"), "{}", result.detail);

        // A memory the user saved has no repo to stay in, so the same link is followed
        node.scope = MemoryScope::Global;
        assert_eq!(verify(&node).status, VerificationStatus::Verified);
        Ok(())
    }

    #[test]
    fn overloads_are_ambiguous_until_qualified_by_id() -> anyhow::Result<()> {
        let dir = tempdir()?;
        let file_path = dir.path().join("Billing.java");
        std::fs::write(
            &file_path,
            "class Billing {\n  void charge(String id) {}\n  void charge(String id, int cents) {}\n}\n",
        )?;
        let base = file_path.to_string_lossy().to_string();

        assert!(hash_target(&format!("{base}#Billing.charge")).is_none());
        let err = target_error(&format!("{base}#Billing.charge")).unwrap();
        assert!(err.contains("Billing.charge, Billing.charge#2"), "{err}");
        assert!(hash_target(&format!("{base}#Billing.charge#2")).is_some());
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

use crate::memory::node::{KnowledgeNode, VerificationResult, VerificationStatus};
use sha2::{Digest, Sha256};
use std::fs;

/// Compute SHA-256 hex digest of a file's contents.
pub fn hash_path(path: &str) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Some(hex::encode(hasher.finalize()))
}

/// Run Jit-V on a single node. Returns a VerificationResult and, if stale,
/// whether the node's `is_stale` flag should be updated in storage.
pub fn verify(node: &KnowledgeNode) -> VerificationResult {
    let Some(ref path) = node.verification_path else {
        return VerificationResult {
            node_id: node.id,
            status: VerificationStatus::Abstract,
            detail: "No verification path — abstract knowledge".into(),
        };
    };

    if !std::path::Path::new(path).exists() {
        return VerificationResult {
            node_id: node.id,
            status: VerificationStatus::StaleMissing,
            detail: format!("Path no longer exists: {path}"),
        };
    }

    match (&node.content_hash, hash_path(path)) {
        (Some(stored), Some(current)) if stored == &current => VerificationResult {
            node_id: node.id,
            status: VerificationStatus::Verified,
            detail: "Hash verified".into(),
        },
        (Some(stored), Some(current)) => VerificationResult {
            node_id: node.id,
            status: VerificationStatus::StaleModified,
            detail: format!(
                "Content changed — stored={}, current={}",
                &stored[..8],
                &current[..8]
            ),
        },
        (None, _) => {
            // Node was saved without a hash — treat as verified (legacy/manual entry)
            VerificationResult {
                node_id: node.id,
                status: VerificationStatus::Verified,
                detail: "No stored hash — path existence confirmed".into(),
            }
        }
        (_, None) => VerificationResult {
            node_id: node.id,
            status: VerificationStatus::StaleMissing,
            detail: format!("Could not read path: {path}"),
        },
    }
}

/// Annotate content with Jit-V status tag for injection into prompt context.
/// Returns None if the node is stale and should be excluded from context.
pub fn annotate(node: &KnowledgeNode, result: &VerificationResult) -> Option<String> {
    let tag = result.status.tag();
    if tag.is_empty() {
        Some(node.content.clone())
    } else {
        // Surface stale nodes with tag — caller decides whether to inject
        Some(format!("{tag} {}", node.content))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{KnowledgeNode, MemoryScope};
    use std::io::Write;
    use tempfile::NamedTempFile;

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
    fn missing_path_is_stale() {
        let node = make_node(Some("/nonexistent/path/xyz.rs".into()), None);
        let result = verify(&node);
        assert_eq!(result.status, VerificationStatus::StaleMissing);
    }

    #[test]
    fn matching_hash_is_verified() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(b"hello world").unwrap();
        let path = f.path().to_str().unwrap().to_string();
        let hash = hash_path(&path).unwrap();
        let node = make_node(Some(path), Some(hash));
        let result = verify(&node);
        assert_eq!(result.status, VerificationStatus::Verified);
    }

    #[test]
    fn wrong_hash_is_stale_modified() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(b"hello world").unwrap();
        let path = f.path().to_str().unwrap().to_string();
        let node = make_node(Some(path), Some("deadbeef00000000".into()));
        let result = verify(&node);
        assert_eq!(result.status, VerificationStatus::StaleModified);
    }
}

pub mod graph;
pub mod lance;

use crate::jitv;
use crate::memory::privacy;
use crate::memory::{Edge, EdgeType, KnowledgeNode, MemoryScope, VerificationStatus};
use anyhow::Result;
use graph::GraphStore;
use lance::VectorStore;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::str::FromStr;
use uuid::Uuid;

pub struct StorageEngine {
    pub graph: GraphStore,
    pub vectors: VectorStore,
}

/// Result of a single recall hit — includes Jit-V annotation and distance.
#[derive(Debug)]
pub struct RecallResult {
    pub node: KnowledgeNode,
    /// Content with stale tag prepended if applicable.
    pub annotated_content: String,
    pub distance: f32,
    pub is_stale: bool,
}

/// Full report from commit_session — every decision is recorded.
#[derive(Debug)]
pub struct CommitReport {
    pub session_id: String,
    pub project_id: String,
    pub promoted: Vec<Uuid>,
    pub rejected: Vec<RejectionRecord>,
}

impl CommitReport {
    pub fn promoted_count(&self) -> usize {
        self.promoted.len()
    }
    pub fn rejected_count(&self) -> usize {
        self.rejected.len()
    }
}

#[derive(Debug)]
pub struct RejectionRecord {
    pub node_id: Uuid,
    pub reason: RejectionReason,
    /// Short snippet of the node content for display
    pub content_preview: String,
    /// Human-readable explanation of why it was rejected
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RejectionReason {
    /// verification_path disappeared from disk
    StaleMissing,
    /// verification_path exists but content hash changed
    StaleModified,
    /// Node was already marked stale in DB before this commit, and re-verify confirms it
    PreviouslyStaleConfirmed,
}

impl RejectionReason {
    pub fn label(&self) -> &'static str {
        match self {
            RejectionReason::StaleMissing => "STALE:MISSING",
            RejectionReason::StaleModified => "STALE:MODIFIED",
            RejectionReason::PreviouslyStaleConfirmed => "STALE:CONFIRMED",
        }
    }
}

impl StorageEngine {
    pub async fn new(data_dir: &str) -> Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        let db_path = format!("{}/knot.db", data_dir);

        let opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", db_path))?
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);

        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(opts)
            .await?;

        let graph = GraphStore::new(pool.clone()).await?;
        let vectors = VectorStore::new(pool).await?;

        Ok(Self { graph, vectors })
    }

    /// Persist a new knowledge node.
    ///
    /// Exit-code gate: if `command_exit_code` is `Some(n)` and `n != 0`, the node is
    /// demoted to Session scope regardless of the requested scope. Only successful
    /// commands earn durable memory.
    pub async fn save(
        &self,
        content: String,
        tags: Vec<String>,
        verification_path: Option<String>,
        scope: MemoryScope,
        command_exit_code: Option<i32>,
        session_id: &str,
    ) -> Result<KnowledgeNode> {
        let effective_scope = match command_exit_code {
            Some(code) if code != 0 => {
                tracing::info!("exit_code={code} → demoting to Session scope");
                MemoryScope::Session(session_id.to_string())
            }
            _ => scope,
        };

        let clean_content = privacy::scrub(&content);

        let content_hash = verification_path
            .as_deref()
            .and_then(jitv::hash_path);

        let node = KnowledgeNode::new(
            clean_content,
            tags,
            verification_path,
            content_hash,
            effective_scope,
        );

        self.graph.insert_node(&node).await?;
        self.vectors.upsert(node.id, &node.content).await?;

        tracing::info!("Saved node {} scope={}", node.id, node.scope.scope_type());
        Ok(node)
    }

    /// Semantic search with Jit-V pass on all candidates.
    ///
    /// Invariants:
    /// - Stale nodes are returned tagged but never have their utility score incremented.
    /// - A node previously marked stale gets a recovery attempt: if re-verify passes,
    ///   the stale flag is cleared in storage before the result is returned.
    pub async fn recall(&self, query: &str, limit: usize) -> Result<Vec<RecallResult>> {
        let candidates = self.vectors.search(query, limit * 2).await?;

        let mut results = Vec::new();
        for (id, distance) in candidates {
            let Some(mut node) = self.graph.get_node(id).await? else {
                continue;
            };

            let vr = jitv::verify(&node);

            match vr.status {
                VerificationStatus::Abstract | VerificationStatus::Verified => {
                    // Optimistic recovery: if node was stale but re-verifies clean, clear the flag.
                    if node.is_stale {
                        self.graph.clear_stale(node.id).await?;
                        node.is_stale = false;
                        tracing::info!("Node {} recovered from stale state", node.id);
                    }
                    // Only clean nodes earn a utility increment.
                    self.graph.increment_utility(node.id).await?;
                }
                _ => {
                    // Stale: persist the flag, never increment utility.
                    if !node.is_stale {
                        self.graph.mark_stale(node.id).await?;
                        node.is_stale = true;
                        tracing::info!("Marked node {} stale: {}", node.id, vr.detail);
                    }
                }
            }

            let annotated = jitv::annotate(&node, &vr)
                .unwrap_or_else(|| node.content.clone());
            let is_stale = vr.status.is_stale();

            results.push(RecallResult {
                node,
                annotated_content: annotated,
                distance,
                is_stale,
            });
        }

        // Non-stale first, then ascending distance (more similar = lower distance).
        results.sort_by(|a, b| {
            a.is_stale
                .cmp(&b.is_stale)
                .then(a.distance.partial_cmp(&b.distance).unwrap())
        });

        results.truncate(limit);
        Ok(results)
    }

    /// Strict firewall: promote Session-scope nodes to Project scope only if they
    /// pass a fresh Jit-V check at promotion time.
    ///
    /// Every node is accounted for — the returned `CommitReport` records every
    /// promotion and every rejection with a reason. Nothing crosses the boundary
    /// silently.
    pub async fn commit_session(
        &self,
        session_id: &str,
        project_id: &str,
    ) -> Result<CommitReport> {
        let session_nodes = self.graph.get_session_nodes(session_id).await?;

        let mut report = CommitReport {
            session_id: session_id.to_string(),
            project_id: project_id.to_string(),
            promoted: Vec::new(),
            rejected: Vec::new(),
        };

        for mut node in session_nodes {
            let vr = jitv::verify(&node);

            if vr.status.is_stale() {
                // Always persist the stale flag, even if it was already set.
                self.graph.mark_stale(node.id).await?;
                node.is_stale = true;

                let reason = match vr.status {
                    VerificationStatus::StaleMissing => RejectionReason::StaleMissing,
                    VerificationStatus::StaleModified => RejectionReason::StaleModified,
                    // Catches the case where is_stale was already true in DB
                    _ => RejectionReason::PreviouslyStaleConfirmed,
                };

                tracing::warn!(
                    "commit_session BLOCKED node {} reason={} detail={}",
                    node.id, reason.label(), vr.detail
                );

                report.rejected.push(RejectionRecord {
                    node_id: node.id,
                    reason,
                    content_preview: node.content.chars().take(80).collect(),
                    detail: vr.detail,
                });
                continue;
            }

            // Optimistic recovery: if a previously-stale node re-verifies clean,
            // allow it through and clear the flag.
            if node.is_stale {
                self.graph.clear_stale(node.id).await?;
                node.is_stale = false;
                tracing::info!("Node {} recovered and will be promoted", node.id);
            }

            node.scope = MemoryScope::Project(project_id.to_string());
            self.graph.update_node(&node).await?;

            tracing::info!("commit_session PROMOTED node {} → project/{project_id}", node.id);
            report.promoted.push(node.id);
        }

        tracing::info!(
            "commit_session complete: {} promoted, {} rejected",
            report.promoted.len(),
            report.rejected.len()
        );

        Ok(report)
    }

    /// Force Jit-V on a specific node. Returns the updated node and a detail string.
    pub async fn jit_verify_node(&self, id: Uuid) -> Result<Option<(KnowledgeNode, String)>> {
        let Some(mut node) = self.graph.get_node(id).await? else {
            return Ok(None);
        };

        let vr = jitv::verify(&node);

        match vr.status {
            VerificationStatus::Abstract | VerificationStatus::Verified => {
                if node.is_stale {
                    self.graph.clear_stale(id).await?;
                    node.is_stale = false;
                }
            }
            _ => {
                if !node.is_stale {
                    self.graph.mark_stale(id).await?;
                    node.is_stale = true;
                }
            }
        }

        let detail = format!(
            "status={:?} tag='{}' detail='{}'",
            vr.status,
            vr.status.tag(),
            vr.detail
        );
        Ok(Some((node, detail)))
    }

    pub async fn link_nodes(
        &self,
        source_id: Uuid,
        target_id: Uuid,
        edge_type: EdgeType,
    ) -> Result<Edge> {
        let edge = Edge::new(source_id, target_id, edge_type);
        self.graph.insert_edge(&edge).await?;
        Ok(edge)
    }

    pub async fn forget(&self, id: Uuid) -> Result<()> {
        self.graph.delete_node(id).await?;
        self.vectors.delete(id).await?;
        Ok(())
    }

    pub async fn list(
        &self,
        scope_type: Option<&str>,
        scope_id: Option<&str>,
        tag_filter: Option<&str>,
    ) -> Result<Vec<KnowledgeNode>> {
        self.graph
            .list_nodes(scope_type, scope_id, tag_filter)
            .await
    }
}

// ── Integration tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    async fn test_engine() -> StorageEngine {
        // Each test gets its own in-memory SQLite — no cross-test contamination.
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .unwrap();
        let graph = GraphStore::new(pool.clone()).await.unwrap();
        let vectors = VectorStore::new(pool).await.unwrap();
        StorageEngine { graph, vectors }
    }

    fn session() -> &'static str {
        "test-session-001"
    }

    // ── Firewall tests ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn commit_promotes_abstract_node() {
        let engine = test_engine().await;
        engine
            .save(
                "Use WAL mode for concurrent SQLite reads".into(),
                vec!["sqlite".into()],
                None, // no verification path → abstract
                MemoryScope::Session(session().into()),
                Some(0),
                session(),
            )
            .await
            .unwrap();

        let report = engine
            .commit_session(session(), "knot-project")
            .await
            .unwrap();

        assert_eq!(report.promoted_count(), 1, "abstract node should be promoted");
        assert_eq!(report.rejected_count(), 0);
    }

    #[tokio::test]
    async fn commit_promotes_node_with_matching_hash() {
        let engine = test_engine().await;
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(b"fn main() {}").unwrap();
        let path = f.path().to_str().unwrap().to_string();

        engine
            .save(
                "main.rs entry point".into(),
                vec!["rust".into()],
                Some(path),
                MemoryScope::Session(session().into()),
                Some(0),
                session(),
            )
            .await
            .unwrap();

        let report = engine
            .commit_session(session(), "knot-project")
            .await
            .unwrap();

        assert_eq!(report.promoted_count(), 1, "file-backed node with correct hash should promote");
        assert_eq!(report.rejected_count(), 0);
    }

    #[tokio::test]
    async fn commit_blocks_node_when_file_modified() {
        let engine = test_engine().await;
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(b"original content").unwrap();
        let path = f.path().to_str().unwrap().to_string();

        engine
            .save(
                "Wisdom about original content".into(),
                vec!["test".into()],
                Some(path.clone()),
                MemoryScope::Session(session().into()),
                Some(0),
                session(),
            )
            .await
            .unwrap();

        // Mutate the file after saving — simulates the file changing on disk.
        std::fs::write(&path, b"MODIFIED content - hash will not match").unwrap();

        let report = engine
            .commit_session(session(), "knot-project")
            .await
            .unwrap();

        assert_eq!(report.promoted_count(), 0, "modified file should be blocked");
        assert_eq!(report.rejected_count(), 1);
        assert_eq!(
            report.rejected[0].reason,
            RejectionReason::StaleModified,
            "should be rejected as StaleModified"
        );
    }

    #[tokio::test]
    async fn commit_blocks_node_when_file_deleted() {
        let engine = test_engine().await;
        let f = NamedTempFile::new().unwrap();
        let path = f.path().to_str().unwrap().to_string();

        engine
            .save(
                "Wisdom about a now-deleted file".into(),
                vec!["test".into()],
                Some(path.clone()),
                MemoryScope::Session(session().into()),
                Some(0),
                session(),
            )
            .await
            .unwrap();

        // Delete the file.
        drop(f);

        let report = engine
            .commit_session(session(), "knot-project")
            .await
            .unwrap();

        assert_eq!(report.promoted_count(), 0, "missing file should be blocked");
        assert_eq!(report.rejected_count(), 1);
        assert_eq!(
            report.rejected[0].reason,
            RejectionReason::StaleMissing,
        );
    }

    #[tokio::test]
    async fn commit_mixed_batch_correct_counts() {
        let engine = test_engine().await;

        // Node 1: abstract — should promote.
        engine
            .save("Abstract wisdom".into(), vec![], None,
                MemoryScope::Session(session().into()), Some(0), session())
            .await.unwrap();

        // Node 2: valid file — should promote.
        let mut f_valid = NamedTempFile::new().unwrap();
        f_valid.write_all(b"valid").unwrap();
        let valid_path = f_valid.path().to_str().unwrap().to_string();
        engine
            .save("Valid file wisdom".into(), vec![], Some(valid_path),
                MemoryScope::Session(session().into()), Some(0), session())
            .await.unwrap();

        // Node 3: file will be deleted — should be rejected.
        let f_gone = NamedTempFile::new().unwrap();
        let gone_path = f_gone.path().to_str().unwrap().to_string();
        engine
            .save("Wisdom about gone file".into(), vec![], Some(gone_path),
                MemoryScope::Session(session().into()), Some(0), session())
            .await.unwrap();
        drop(f_gone);

        // Node 4: file will be mutated — should be rejected.
        let mut f_mut = NamedTempFile::new().unwrap();
        f_mut.write_all(b"before").unwrap();
        let mut_path = f_mut.path().to_str().unwrap().to_string();
        engine
            .save("Wisdom about mutated file".into(), vec![], Some(mut_path.clone()),
                MemoryScope::Session(session().into()), Some(0), session())
            .await.unwrap();
        std::fs::write(&mut_path, b"after").unwrap();

        let report = engine
            .commit_session(session(), "knot-project")
            .await
            .unwrap();

        assert_eq!(report.promoted_count(), 2, "2 of 4 nodes should promote");
        assert_eq!(report.rejected_count(), 2, "2 of 4 nodes should be rejected");
    }

    // ── Recall utility-score invariant ────────────────────────────────────────

    #[tokio::test]
    async fn recall_does_not_increment_utility_for_stale_node() {
        let engine = test_engine().await;

        let mut f = NamedTempFile::new().unwrap();
        f.write_all(b"initial").unwrap();
        let path = f.path().to_str().unwrap().to_string();

        engine
            .save(
                "cosine similarity distance threshold".into(),
                vec![],
                Some(path.clone()),
                MemoryScope::Global,
                None,
                session(),
            )
            .await
            .unwrap();

        // Make the file stale.
        std::fs::write(&path, b"modified").unwrap();

        let results = engine.recall("cosine similarity", 5).await.unwrap();
        assert!(!results.is_empty());

        // Confirm the node is returned as stale.
        let hit = &results[0];
        assert!(hit.is_stale, "node should be marked stale in recall result");

        // Confirm utility score was NOT incremented (stays at 0.5).
        let node = engine.graph.get_node(hit.node.id).await.unwrap().unwrap();
        assert!(
            (node.utility_score - 0.5).abs() < 1e-4,
            "utility score must not increase for stale recall: got {}",
            node.utility_score
        );
    }

    // ── Stale recovery ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn stale_node_recovers_when_file_restored() {
        let engine = test_engine().await;

        let mut f = NamedTempFile::new().unwrap();
        let original = b"fn answer() -> u32 { 42 }";
        f.write_all(original).unwrap();
        let path = f.path().to_str().unwrap().to_string();

        engine
            .save(
                "answer function returns 42".into(),
                vec![],
                Some(path.clone()),
                MemoryScope::Global,
                None,
                session(),
            )
            .await
            .unwrap();

        // Corrupt the file → marks node stale on first recall.
        std::fs::write(&path, b"corrupted").unwrap();
        let results = engine.recall("answer function", 5).await.unwrap();
        assert!(results[0].is_stale);

        // Restore the original file content.
        std::fs::write(&path, original).unwrap();

        let results = engine.recall("answer function", 5).await.unwrap();
        assert!(
            !results[0].is_stale,
            "node should recover when file is restored"
        );

        // Confirm the DB flag was cleared.
        let node = engine.graph.get_node(results[0].node.id).await.unwrap().unwrap();
        assert!(!node.is_stale, "DB is_stale flag must be cleared on recovery");
    }
}

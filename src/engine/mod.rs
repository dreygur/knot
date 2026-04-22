pub mod graph;
pub mod lance;
mod memory_ops;
mod skill_ops;
mod types;

pub use types::*;

use anyhow::Result;
use graph::GraphStore;
use lance::VectorStore;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::str::FromStr;

pub struct StorageEngine {
    pub graph: GraphStore,
    pub vectors: VectorStore,
}

impl StorageEngine {
    pub async fn new(data_dir: &str) -> Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        let db_path = format!("{}/knot.db", data_dir);

        let opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", db_path))?
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            // Retry-on-Busy: wait up to 5 s before returning SQLITE_BUSY.
            // Prevents panics under parallel MCP tool calls.
            .busy_timeout(std::time::Duration::from_secs(5));

        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(opts)
            .await?;

        let graph = GraphStore::new(pool.clone()).await?;
        let vectors = VectorStore::new(pool).await?;

        Ok(Self { graph, vectors })
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

    fn req(
        content: &str,
        tags: Vec<&str>,
        path: Option<String>,
        scope: crate::memory::MemoryScope,
        exit_code: Option<i32>,
    ) -> SaveRequest {
        SaveRequest {
            content: content.into(),
            tags: tags.into_iter().map(String::from).collect(),
            verification_path: path,
            scope,
            command_exit_code: exit_code,
            session_id: session().into(),
            parent_id: None,
            origin_agent: None,
        }
    }

    // ── Firewall tests ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn commit_promotes_abstract_node() {
        let engine = test_engine().await;
        engine
            .save(req(
                "Use WAL mode for concurrent SQLite reads",
                vec!["sqlite"],
                None,
                crate::memory::MemoryScope::Session(session().into()),
                Some(0),
            ))
            .await
            .unwrap();

        let report = engine
            .commit_session(session(), "knot-project")
            .await
            .unwrap();

        assert_eq!(
            report.promoted_count(),
            1,
            "abstract node should be promoted"
        );
        assert_eq!(report.rejected_count(), 0);
    }

    #[tokio::test]
    async fn commit_promotes_node_with_matching_hash() {
        let engine = test_engine().await;
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(b"fn main() {}").unwrap();
        let path = f.path().to_str().unwrap().to_string();

        engine
            .save(req(
                "main.rs entry point",
                vec!["rust"],
                Some(path),
                crate::memory::MemoryScope::Session(session().into()),
                Some(0),
            ))
            .await
            .unwrap();

        let report = engine
            .commit_session(session(), "knot-project")
            .await
            .unwrap();

        assert_eq!(
            report.promoted_count(),
            1,
            "file-backed node with correct hash should promote"
        );
        assert_eq!(report.rejected_count(), 0);
    }

    #[tokio::test]
    async fn commit_blocks_node_when_file_modified() {
        let engine = test_engine().await;
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(b"original content").unwrap();
        let path = f.path().to_str().unwrap().to_string();

        engine
            .save(req(
                "Wisdom about original content",
                vec!["test"],
                Some(path.clone()),
                crate::memory::MemoryScope::Session(session().into()),
                Some(0),
            ))
            .await
            .unwrap();

        // Mutate the file after saving — simulates the file changing on disk.
        std::fs::write(&path, b"MODIFIED content - hash will not match").unwrap();

        let report = engine
            .commit_session(session(), "knot-project")
            .await
            .unwrap();

        assert_eq!(
            report.promoted_count(),
            0,
            "modified file should be blocked"
        );
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
            .save(req(
                "Wisdom about a now-deleted file",
                vec!["test"],
                Some(path.clone()),
                crate::memory::MemoryScope::Session(session().into()),
                Some(0),
            ))
            .await
            .unwrap();

        drop(f);

        let report = engine
            .commit_session(session(), "knot-project")
            .await
            .unwrap();

        assert_eq!(report.promoted_count(), 0, "missing file should be blocked");
        assert_eq!(report.rejected_count(), 1);
        assert_eq!(report.rejected[0].reason, RejectionReason::StaleMissing);
    }

    #[tokio::test]
    async fn commit_mixed_batch_correct_counts() {
        let engine = test_engine().await;

        // Node 1: abstract — should promote.
        engine
            .save(req(
                "Abstract wisdom",
                vec![],
                None,
                crate::memory::MemoryScope::Session(session().into()),
                Some(0),
            ))
            .await
            .unwrap();

        // Node 2: valid file — should promote.
        let mut f_valid = NamedTempFile::new().unwrap();
        f_valid.write_all(b"valid").unwrap();
        let valid_path = f_valid.path().to_str().unwrap().to_string();
        engine
            .save(req(
                "Valid file wisdom",
                vec![],
                Some(valid_path),
                crate::memory::MemoryScope::Session(session().into()),
                Some(0),
            ))
            .await
            .unwrap();

        // Node 3: file will be deleted — should be rejected.
        let f_gone = NamedTempFile::new().unwrap();
        let gone_path = f_gone.path().to_str().unwrap().to_string();
        engine
            .save(req(
                "Wisdom about gone file",
                vec![],
                Some(gone_path),
                crate::memory::MemoryScope::Session(session().into()),
                Some(0),
            ))
            .await
            .unwrap();
        drop(f_gone);

        // Node 4: file will be mutated — should be rejected.
        let mut f_mut = NamedTempFile::new().unwrap();
        f_mut.write_all(b"before").unwrap();
        let mut_path = f_mut.path().to_str().unwrap().to_string();
        engine
            .save(req(
                "Wisdom about mutated file",
                vec![],
                Some(mut_path.clone()),
                crate::memory::MemoryScope::Session(session().into()),
                Some(0),
            ))
            .await
            .unwrap();
        std::fs::write(&mut_path, b"after").unwrap();

        let report = engine
            .commit_session(session(), "knot-project")
            .await
            .unwrap();

        assert_eq!(report.promoted_count(), 2, "2 of 4 nodes should promote");
        assert_eq!(
            report.rejected_count(),
            2,
            "2 of 4 nodes should be rejected"
        );
    }

    // ── Recall utility-score invariant ────────────────────────────────────────

    #[tokio::test]
    async fn recall_does_not_increment_utility_for_stale_node() {
        let engine = test_engine().await;

        let mut f = NamedTempFile::new().unwrap();
        f.write_all(b"initial").unwrap();
        let path = f.path().to_str().unwrap().to_string();

        engine
            .save(req(
                "cosine similarity distance threshold",
                vec![],
                Some(path.clone()),
                crate::memory::MemoryScope::Global,
                None,
            ))
            .await
            .unwrap();

        std::fs::write(&path, b"modified").unwrap();

        let results = engine.recall("cosine similarity", 5).await.unwrap();
        assert!(!results.is_empty());

        let hit = &results[0];
        assert!(hit.is_stale, "node should be marked stale in recall result");

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
            .save(req(
                "answer function returns 42",
                vec![],
                Some(path.clone()),
                crate::memory::MemoryScope::Global,
                None,
            ))
            .await
            .unwrap();

        std::fs::write(&path, b"corrupted").unwrap();
        let results = engine.recall("answer function", 5).await.unwrap();
        assert!(results[0].is_stale);

        std::fs::write(&path, original).unwrap();

        let results = engine.recall("answer function", 5).await.unwrap();
        assert!(
            !results[0].is_stale,
            "node should recover when file is restored"
        );

        let node = engine
            .graph
            .get_node(results[0].node.id)
            .await
            .unwrap()
            .unwrap();
        assert!(
            !node.is_stale,
            "DB is_stale flag must be cleared on recovery"
        );
    }

    // ── delete_wisdom ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn delete_wisdom_removes_node_and_reparents_children() {
        let engine = test_engine().await;

        let parent = engine
            .save(req(
                "parent node",
                vec![],
                None,
                crate::memory::MemoryScope::Global,
                None,
            ))
            .await
            .unwrap();

        let child_req = SaveRequest {
            parent_id: Some(parent.id),
            ..req(
                "child node",
                vec![],
                None,
                crate::memory::MemoryScope::Global,
                None,
            )
        };
        let child = engine.save(child_req).await.unwrap();
        assert_eq!(child.parent_id, Some(parent.id));

        let report = engine.delete_wisdom(parent.id).await.unwrap().unwrap();
        assert_eq!(report.children_reparented, 1);
        assert!(
            engine.graph.get_node(parent.id).await.unwrap().is_none(),
            "parent must be gone"
        );

        let child_after = engine.graph.get_node(child.id).await.unwrap().unwrap();
        assert_eq!(
            child_after.parent_id, None,
            "child must be re-parented to NULL"
        );
    }

    #[tokio::test]
    async fn delete_wisdom_returns_none_for_missing_node() {
        let engine = test_engine().await;
        let absent = uuid::Uuid::new_v4();
        assert!(engine.delete_wisdom(absent).await.unwrap().is_none());
    }

    // ── delete_skill ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn delete_skill_removes_low_utility_skill() {
        let engine = test_engine().await;
        engine
            .save_skill(
                "low-util".into(),
                "desc".into(),
                vec![],
                vec![],
                "true".into(),
                None,
            )
            .await
            .unwrap();

        let result = engine.delete_skill("low-util", false).await.unwrap();
        assert!(matches!(result, DeleteSkillResult::Deleted));
        assert!(engine.graph.get_skill("low-util").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn delete_skill_blocks_high_utility_without_force() {
        let engine = test_engine().await;
        engine
            .save_skill(
                "hot-skill".into(),
                "desc".into(),
                vec![],
                vec![],
                "true".into(),
                None,
            )
            .await
            .unwrap();

        for _ in 0..11 {
            engine
                .graph
                .increment_skill_success("hot-skill")
                .await
                .unwrap();
        }

        let result = engine.delete_skill("hot-skill", false).await.unwrap();
        assert!(matches!(
            result,
            DeleteSkillResult::HighUtilityBlocked { .. }
        ));
        assert!(
            engine.graph.get_skill("hot-skill").await.unwrap().is_some(),
            "skill must survive"
        );
    }

    #[tokio::test]
    async fn delete_skill_force_bypasses_high_utility_gate() {
        let engine = test_engine().await;
        engine
            .save_skill(
                "hot-skill".into(),
                "desc".into(),
                vec![],
                vec![],
                "true".into(),
                None,
            )
            .await
            .unwrap();
        for _ in 0..11 {
            engine
                .graph
                .increment_skill_success("hot-skill")
                .await
                .unwrap();
        }

        let result = engine.delete_skill("hot-skill", true).await.unwrap();
        assert!(matches!(result, DeleteSkillResult::Deleted));
        assert!(engine.graph.get_skill("hot-skill").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn delete_skill_not_found() {
        let engine = test_engine().await;
        let result = engine.delete_skill("ghost-skill", false).await.unwrap();
        assert!(matches!(result, DeleteSkillResult::NotFound));
    }
}

pub mod graph;
pub mod hybrid;
pub mod lance;
pub mod learnings;
mod memory_ops;
mod skill_ops;
mod types;

pub use types::*;

use anyhow::Result;
use graph::GraphStore;
use lance::VectorStore;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

use crate::ast::resolve::{source_fingerprint, SourceFingerprint};
use crate::ast::CodeGraph;

/// Minimum time between source fingerprint scans during recall bursts.
const INDEX_RECHECK_INTERVAL: Duration = Duration::from_secs(5);
/// Query tokens naming more symbols than this (e.g. `new`) are too vague to seed PageRank.
const MAX_SEEDS_PER_TOKEN: usize = 8;

/// Code graph for the repo the server runs in, rebuilt only when sources change.
struct CodeIndex {
    root: PathBuf,
    fingerprint: SourceFingerprint,
    checked_at: Instant,
    graph: Arc<CodeGraph>,
}

pub struct StorageEngine {
    pub graph: GraphStore,
    pub vectors: VectorStore,
    centrality: Arc<RwLock<HashMap<String, f64>>>,
    code_index: Arc<RwLock<Option<CodeIndex>>>,
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
        let centrality = Arc::new(RwLock::new(HashMap::new()));

        Ok(Self {
            graph,
            vectors,
            centrality,
            code_index: Arc::new(RwLock::new(None)),
        })
    }

    pub async fn set_centrality(&self, scores: HashMap<String, f64>) {
        let mut lock = self.centrality.write().await;
        *lock = scores;
    }

    pub async fn get_centrality_snapshot(&self) -> HashMap<String, f64> {
        let lock = self.centrality.read().await;
        lock.clone()
    }

    /// Make sure the cached code graph and global PageRank reflect `repo_root`.
    ///
    /// Sources are only stat'ed (at most every few seconds) to detect changes; the graph
    /// is re-parsed only when the file set, sizes or mtimes differ from the last build.
    pub async fn refresh_centrality(&self, repo_root: &Path) -> Result<()> {
        {
            let index = self.code_index.read().await;
            if let Some(idx) = index.as_ref() {
                if idx.root == repo_root && idx.checked_at.elapsed() < INDEX_RECHECK_INTERVAL {
                    return Ok(());
                }
            }
        }

        let root = repo_root.to_path_buf();
        let fingerprint = tokio::task::spawn_blocking(move || source_fingerprint(&root)).await??;
        {
            let mut index = self.code_index.write().await;
            if let Some(idx) = index.as_mut() {
                if idx.root == repo_root && idx.fingerprint == fingerprint {
                    idx.checked_at = Instant::now();
                    return Ok(());
                }
            }
        }

        let root = repo_root.to_path_buf();
        let cache_root = crate::ask::index_root(&root);
        let (graph, scores) = tokio::task::spawn_blocking(move || -> Result<_> {
            let (corpus, _) = crate::ask::load_corpus(&root, Some(&cache_root))?;
            let graph = corpus.graph;
            let scores = crate::ast::compute_pagerank(&graph);
            Ok((graph, scores))
        })
        .await??;

        self.set_centrality(scores).await;
        *self.code_index.write().await = Some(CodeIndex {
            root: repo_root.to_path_buf(),
            fingerprint,
            checked_at: Instant::now(),
            graph: Arc::new(graph),
        });
        Ok(())
    }

    /// Centrality scores for a recall query, plus the repo root they are keyed against.
    ///
    /// Symbols named in the query seed a Personalized PageRank, so memories about code
    /// near those symbols rank higher. Without named symbols, global PageRank is used.
    pub(crate) async fn query_centrality(&self, query: &str) -> (HashMap<String, f64>, Option<PathBuf>) {
        let index = self.code_index.read().await;
        let Some(idx) = index.as_ref() else {
            return (self.get_centrality_snapshot().await, None);
        };

        let mut seeds: Vec<String> = query
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .filter_map(|token| idx.graph.name_to_ids.get(token))
            .filter(|ids| ids.len() <= MAX_SEEDS_PER_TOKEN)
            .flatten()
            .cloned()
            .collect();
        seeds.sort();
        seeds.dedup();

        let root = Some(idx.root.clone());
        if seeds.is_empty() {
            drop(index);
            return (self.get_centrality_snapshot().await, root);
        }

        let graph = Arc::clone(&idx.graph);
        drop(index);
        let scores = tokio::task::spawn_blocking(move || crate::ast::compute_personalized_pagerank(&graph, &seeds))
            .await
            .unwrap_or_default();
        (scores, root)
    }
}

// ── Integration tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    async fn test_engine() -> StorageEngine {
        // Each test gets its own in-memory SQLite - no cross-test contamination.
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
        let centrality = Arc::new(RwLock::new(HashMap::new()));
        StorageEngine {
            graph,
            vectors,
            centrality,
            code_index: Arc::new(RwLock::new(None)),
        }
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
            .commit_session_to(session(), "knot-project", None)
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
            .commit_session_to(session(), "knot-project", None)
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

        // Mutate the file after saving - simulates the file changing on disk.
        std::fs::write(&path, b"MODIFIED content - hash will not match").unwrap();

        let report = engine
            .commit_session_to(session(), "knot-project", None)
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
            .commit_session_to(session(), "knot-project", None)
            .await
            .unwrap();

        assert_eq!(report.promoted_count(), 0, "missing file should be blocked");
        assert_eq!(report.rejected_count(), 1);
        assert_eq!(report.rejected[0].reason, RejectionReason::StaleMissing);
    }

    #[tokio::test]
    async fn commit_mixed_batch_correct_counts() {
        let engine = test_engine().await;

        // Node 1: abstract - should promote.
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

        // Node 2: valid file - should promote.
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

        // Node 3: file will be deleted - should be rejected.
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

        // Node 4: file will be mutated - should be rejected.
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
            .commit_session_to(session(), "knot-project", None)
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

    #[tokio::test]
    async fn sync_repo_learnings_indexes_and_updates_files() {
        let engine = test_engine().await;
        let dir = tempfile::tempdir().unwrap();
        let learnings = dir.path().join(".knot").join("learnings");
        std::fs::create_dir_all(&learnings).unwrap();

        let id = uuid::Uuid::new_v4();
        let write = |decided: &str| {
            std::fs::write(
                learnings.join("2026-10-26_wal.md"),
                format!(
                    "---\nid: {id}\ntitle: \"Use WAL journaling\"\nauthor: \"a\"\ndate: \"2026-10-26\"\nscope: \"project\"\ntouches:\n  - \"src/db.rs\"\n---\n\n## Decided\n{decided}\n"
                ),
            )
            .unwrap();
        };

        write("Enable WAL so readers never block the writer.");
        assert_eq!(engine.sync_repo_learnings(dir.path()).await.unwrap(), 1);

        let node = engine.graph.get_node(id).await.unwrap().unwrap();
        assert!(node.content.starts_with("Use WAL journaling"));
        assert!(node.scope.scope_type() == "project");
        let expected_path = std::fs::canonicalize(dir.path()).unwrap().join("src/db.rs").to_string_lossy().to_string();
        assert_eq!(node.verification_path.as_deref(), Some(expected_path.as_str()));

        // Re-sync of an unchanged file must not fail on the existing primary key.
        assert_eq!(engine.sync_repo_learnings(dir.path()).await.unwrap(), 1);

        write("Enable WAL with synchronous=NORMAL for checkpoint throughput.");
        assert_eq!(engine.sync_repo_learnings(dir.path()).await.unwrap(), 1);
        let updated = engine.graph.get_node(id).await.unwrap().unwrap();
        assert!(updated.content.contains("synchronous=NORMAL"));

        let repo = std::fs::canonicalize(dir.path()).unwrap();
        let results = engine.recall_in("checkpoint throughput", 5, Some(&repo)).await.unwrap();
        let hit = results.iter().find(|r| r.node.id == id).expect("learning recalled inside its repo");
        assert!(hit.annotated_content.starts_with("[REPO LEARNING"), "{}", hit.annotated_content);

        // Another repo, or no repo, never sees it
        let elsewhere = tempfile::tempdir().unwrap();
        let other = std::fs::canonicalize(elsewhere.path()).unwrap();
        for current in [Some(other.as_path()), None] {
            let results = engine.recall_in("checkpoint throughput", 5, current).await.unwrap();
            assert!(!results.iter().any(|r| r.node.id == id), "leaked into {current:?}");
        }
    }

    fn write_learning(dir: &std::path::Path, id: uuid::Uuid, touches: &str) {
        let learnings = dir.join(".knot").join("learnings");
        std::fs::create_dir_all(&learnings).unwrap();
        std::fs::write(
            learnings.join(format!("{id}.md")),
            format!(
                "---\nid: {id}\ntitle: \"Planted\"\nauthor: \"x\"\ndate: \"2026-10-27\"\nscope: \"project\"\ntouches:\n  - \"{touches}\"\n---\n\nPlanted because reasons.\n"
            ),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn sync_ignores_touches_outside_repo() {
        let engine = test_engine().await;
        let outside = tempfile::NamedTempFile::new().unwrap();
        let dir = tempfile::tempdir().unwrap();

        let abs_id = uuid::Uuid::new_v4();
        write_learning(dir.path(), abs_id, &outside.path().to_string_lossy());
        let up_id = uuid::Uuid::new_v4();
        write_learning(dir.path(), up_id, "../../etc/hosts");

        engine.sync_repo_learnings(dir.path()).await.unwrap();
        for id in [abs_id, up_id] {
            let node = engine.graph.get_node(id).await.unwrap().unwrap();
            assert_eq!(node.verification_path, None, "touch escaped the repo: {:?}", node.verification_path);
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn sync_ignores_touches_through_symlinks_out_of_repo() {
        let engine = test_engine().await;
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("id_rsa"), "secret").unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();

        let id = uuid::Uuid::new_v4();
        write_learning(dir.path(), id, "link/id_rsa");
        engine.sync_repo_learnings(dir.path()).await.unwrap();

        let node = engine.graph.get_node(id).await.unwrap().unwrap();
        assert_eq!(node.verification_path, None, "symlink escaped: {:?}", node.verification_path);
    }

    #[tokio::test]
    async fn sync_keeps_same_named_repos_apart() {
        let engine = test_engine().await;
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let (repo_a, repo_b) = (a.path().join("app"), b.path().join("app"));

        let id = uuid::Uuid::new_v4();
        write_learning(&repo_a, id, "src/lib.rs");
        engine.sync_repo_learnings(&repo_a).await.unwrap();
        let original = engine.graph.get_node(id).await.unwrap().unwrap();

        let learnings = repo_b.join(".knot").join("learnings");
        std::fs::create_dir_all(&learnings).unwrap();
        std::fs::write(
            learnings.join("planted.md"),
            format!("---\nid: {id}\ntitle: \"Hijack\"\nauthor: \"m\"\ndate: \"2026-10-27\"\nscope: \"project\"\n---\n\nHijacked because yes.\n"),
        )
        .unwrap();
        engine.sync_repo_learnings(&repo_b).await.unwrap();

        let after = engine.graph.get_node(id).await.unwrap().unwrap();
        assert_eq!(after.content, original.content);
    }

    #[tokio::test]
    async fn sync_does_not_overwrite_memories_from_other_scopes() {
        let engine = test_engine().await;
        let global = engine
            .save(req("deploy key lives in vault path secret/ci", vec![], None, crate::memory::MemoryScope::Global, None))
            .await
            .unwrap();

        let dir = tempfile::tempdir().unwrap();
        write_learning(dir.path(), global.id, "src/lib.rs");
        engine.sync_repo_learnings(dir.path()).await.unwrap();

        let after = engine.graph.get_node(global.id).await.unwrap().unwrap();
        assert_eq!(after.content, global.content);
        assert_eq!(after.scope.scope_type(), "global");
    }

    #[tokio::test]
    async fn hybrid_recall_retrieves_exact_lexical_keywords() {
        let engine = test_engine().await;

        let node1 = engine
            .save(req(
                "ConfigWatcher::reload parses yaml and updates worker threads",
                vec!["config", "concurrency"],
                None,
                crate::memory::MemoryScope::Project("p1".into()),
                None,
            ))
            .await
            .unwrap();

        let _node2 = engine
            .save(req(
                "general background task scheduler periodic runner",
                vec!["scheduler"],
                None,
                crate::memory::MemoryScope::Project("p1".into()),
                None,
            ))
            .await
            .unwrap();

        let results = engine
            .recall("ConfigWatcher::reload", 5)
            .await
            .unwrap();

        assert!(!results.is_empty());
        assert_eq!(results[0].node.id, node1.id);
        assert!(results[0].distance < 0.65);
    }

    #[tokio::test]
    async fn hybrid_recall_centrality_boosts_hub_node() {
        let engine = test_engine().await;

        let hub_path = "src/core/hub.rs#Dispatcher";
        let leaf_path = "src/util/leaf.rs#helper";

        let hub_node = engine
            .save(req(
                "event dispatch router pipeline",
                vec!["router"],
                Some(hub_path.to_string()),
                crate::memory::MemoryScope::Project("p1".into()),
                None,
            ))
            .await
            .unwrap();

        let leaf_node = engine
            .save(req(
                "event dispatch router pipeline",
                vec!["router"],
                Some(leaf_path.to_string()),
                crate::memory::MemoryScope::Project("p1".into()),
                None,
            ))
            .await
            .unwrap();

        let mut centrality = HashMap::new();
        centrality.insert("src/core/hub.rs::Dispatcher".to_string(), 0.85);
        centrality.insert("src/util/leaf.rs::helper".to_string(), 0.05);
        engine.set_centrality(centrality).await;

        let results = engine
            .recall("event dispatch router", 5)
            .await
            .unwrap();

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].node.id, hub_node.id);
        assert_eq!(results[1].node.id, leaf_node.id);
        assert!(results[0].distance < results[1].distance);
    }

    #[tokio::test]
    async fn recall_uses_real_code_graph_and_personalized_seeds() {
        let engine = test_engine().await;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/lib.rs"),
            "pub fn dispatch() {}\npub fn a() { dispatch(); }\npub fn b() { dispatch(); }\npub fn c() { dispatch(); }\npub fn lonely() {}\n",
        )
        .unwrap();

        let path = |sym: &str| format!("{}#{sym}", root.join("src/lib.rs").to_string_lossy());
        let hub = engine
            .save(req("queue flush ordering rules", vec![], Some(path("dispatch")), crate::memory::MemoryScope::Project("p1".into()), None))
            .await
            .unwrap();
        let leaf = engine
            .save(req("queue flush ordering rules", vec![], Some(path("lonely")), crate::memory::MemoryScope::Project("p1".into()), None))
            .await
            .unwrap();

        engine.refresh_centrality(root).await.unwrap();
        let results = engine.recall("queue flush ordering", 5).await.unwrap();
        assert_eq!(results[0].node.id, hub.id, "graph hub should outrank an isolated symbol");

        // Naming `lonely` seeds PageRank there, so it outranks the global hub.
        let (global, _) = engine.query_centrality("queue flush").await;
        let (seeded, seeded_root) = engine.query_centrality("why is lonely unused").await;
        let score = |scores: &HashMap<String, f64>, sym: &str| {
            crate::engine::hybrid::resolve_centrality(scores, Some(&path(sym)), seeded_root.as_deref())
        };
        assert!(score(&global, "dispatch") > score(&global, "lonely"));
        assert!(score(&seeded, "lonely") > score(&seeded, "dispatch"));
        let results = engine.recall("queue flush ordering lonely", 5).await.unwrap();
        assert_eq!(results[0].node.id, leaf.id);

        // Unchanged sources must reuse the cached graph.
        let before = Arc::as_ptr(&engine.code_index.read().await.as_ref().unwrap().graph);
        engine.code_index.write().await.as_mut().unwrap().checked_at -= INDEX_RECHECK_INTERVAL;
        engine.refresh_centrality(root).await.unwrap();
        let after = Arc::as_ptr(&engine.code_index.read().await.as_ref().unwrap().graph);
        assert_eq!(before, after);
    }
}

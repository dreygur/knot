use crate::memory::{Edge, EdgeType, KnowledgeNode, MemoryScope};
use anyhow::Result;
use chrono::Utc;
use sqlx::{sqlite::SqlitePool, Row};
use uuid::Uuid;

pub struct GraphStore {
    pool: SqlitePool,
}

impl GraphStore {
    pub async fn new(pool: SqlitePool) -> Result<Self> {
        let store = Self { pool };
        store.migrate().await?;
        Ok(store)
    }

    async fn migrate(&self) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS nodes (
                id              TEXT PRIMARY KEY,
                content         TEXT NOT NULL,
                tags            TEXT NOT NULL,
                verification_path TEXT,
                content_hash    TEXT,
                utility_score   REAL NOT NULL DEFAULT 0.5,
                scope_type      TEXT NOT NULL,
                scope_id        TEXT,
                is_stale        INTEGER NOT NULL DEFAULT 0,
                created_at      TEXT NOT NULL,
                updated_at      TEXT NOT NULL
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS edges (
                id          TEXT PRIMARY KEY,
                source_id   TEXT NOT NULL,
                target_id   TEXT NOT NULL,
                edge_type   TEXT NOT NULL,
                created_at  TEXT NOT NULL,
                FOREIGN KEY (source_id) REFERENCES nodes(id) ON DELETE CASCADE,
                FOREIGN KEY (target_id) REFERENCES nodes(id) ON DELETE CASCADE
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_nodes_scope ON nodes(scope_type, scope_id);")
            .execute(&self.pool)
            .await?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_edges_source ON edges(source_id);")
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    pub async fn insert_node(&self, node: &KnowledgeNode) -> Result<()> {
        let tags_json = serde_json::to_string(&node.tags)?;
        sqlx::query(
            r#"
            INSERT INTO nodes
                (id, content, tags, verification_path, content_hash,
                 utility_score, scope_type, scope_id, is_stale, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(node.id.to_string())
        .bind(&node.content)
        .bind(&tags_json)
        .bind(&node.verification_path)
        .bind(&node.content_hash)
        .bind(node.utility_score)
        .bind(node.scope.scope_type())
        .bind(node.scope.scope_id())
        .bind(node.is_stale as i32)
        .bind(node.created_at.to_rfc3339())
        .bind(node.updated_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn update_node(&self, node: &KnowledgeNode) -> Result<()> {
        let tags_json = serde_json::to_string(&node.tags)?;
        sqlx::query(
            r#"
            UPDATE nodes SET
                content = ?, tags = ?, verification_path = ?, content_hash = ?,
                utility_score = ?, scope_type = ?, scope_id = ?,
                is_stale = ?, updated_at = ?
            WHERE id = ?
            "#,
        )
        .bind(&node.content)
        .bind(&tags_json)
        .bind(&node.verification_path)
        .bind(&node.content_hash)
        .bind(node.utility_score)
        .bind(node.scope.scope_type())
        .bind(node.scope.scope_id())
        .bind(node.is_stale as i32)
        .bind(node.updated_at.to_rfc3339())
        .bind(node.id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_node(&self, id: Uuid) -> Result<Option<KnowledgeNode>> {
        let row = sqlx::query(
            "SELECT id, content, tags, verification_path, content_hash, utility_score, \
             scope_type, scope_id, is_stale, created_at, updated_at \
             FROM nodes WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;

        row.map(|r| row_to_node(&r)).transpose()
    }

    pub async fn delete_node(&self, id: Uuid) -> Result<()> {
        sqlx::query("DELETE FROM nodes WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn list_nodes(
        &self,
        scope_type: Option<&str>,
        scope_id: Option<&str>,
        tag_filter: Option<&str>,
    ) -> Result<Vec<KnowledgeNode>> {
        let rows = sqlx::query(
            "SELECT id, content, tags, verification_path, content_hash, utility_score, \
             scope_type, scope_id, is_stale, created_at, updated_at \
             FROM nodes ORDER BY utility_score DESC",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut nodes: Vec<KnowledgeNode> = rows
            .iter()
            .filter_map(|r| row_to_node(r).ok())
            .filter(|n| {
                if let Some(st) = scope_type {
                    if n.scope.scope_type() != st {
                        return false;
                    }
                }
                if let Some(sid) = scope_id {
                    if n.scope.scope_id() != Some(sid) {
                        return false;
                    }
                }
                if let Some(tag) = tag_filter {
                    if !n.tags.iter().any(|t| t.contains(tag)) {
                        return false;
                    }
                }
                true
            })
            .collect();

        nodes.sort_by(|a, b| {
            a.is_stale
                .cmp(&b.is_stale)
                .then(b.utility_score.partial_cmp(&a.utility_score).unwrap())
        });
        Ok(nodes)
    }

    /// Fetch all Session-scope nodes for a given session_id
    pub async fn get_session_nodes(&self, session_id: &str) -> Result<Vec<KnowledgeNode>> {
        self.list_nodes(Some("session"), Some(session_id), None)
            .await
    }

    pub async fn insert_edge(&self, edge: &Edge) -> Result<()> {
        sqlx::query(
            "INSERT INTO edges (id, source_id, target_id, edge_type, created_at) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(edge.id.to_string())
        .bind(edge.source_id.to_string())
        .bind(edge.target_id.to_string())
        .bind(edge.edge_type.as_str())
        .bind(edge.created_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_edges_from(&self, source_id: Uuid) -> Result<Vec<Edge>> {
        let rows = sqlx::query(
            "SELECT id, source_id, target_id, edge_type, created_at \
             FROM edges WHERE source_id = ?",
        )
        .bind(source_id.to_string())
        .fetch_all(&self.pool)
        .await?;

        rows.iter()
            .map(|r| {
                let id: String = r.get("id");
                let src: String = r.get("source_id");
                let tgt: String = r.get("target_id");
                let et: String = r.get("edge_type");
                let ca: String = r.get("created_at");
                Ok(Edge {
                    id: Uuid::parse_str(&id)?,
                    source_id: Uuid::parse_str(&src)?,
                    target_id: Uuid::parse_str(&tgt)?,
                    edge_type: EdgeType::from_str(&et)
                        .ok_or_else(|| anyhow::anyhow!("Unknown edge type: {et}"))?,
                    created_at: ca.parse()?,
                })
            })
            .collect()
    }

    pub async fn mark_stale(&self, id: Uuid) -> Result<()> {
        sqlx::query("UPDATE nodes SET is_stale = 1, updated_at = ? WHERE id = ?")
            .bind(Utc::now().to_rfc3339())
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Clear the stale flag — called when Jit-V re-verifies a previously-stale node.
    pub async fn clear_stale(&self, id: Uuid) -> Result<()> {
        sqlx::query("UPDATE nodes SET is_stale = 0, updated_at = ? WHERE id = ?")
            .bind(Utc::now().to_rfc3339())
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn increment_utility(&self, id: Uuid) -> Result<()> {
        sqlx::query(
            "UPDATE nodes SET utility_score = MIN(1.0, utility_score + 0.05), updated_at = ? \
             WHERE id = ?",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

fn row_to_node(r: &sqlx::sqlite::SqliteRow) -> Result<KnowledgeNode> {
    let id_str: String = r.get("id");
    let tags_json: String = r.get("tags");
    let scope_type: String = r.get("scope_type");
    let scope_id: Option<String> = r.get("scope_id");
    let created_at_str: String = r.get("created_at");
    let updated_at_str: String = r.get("updated_at");

    let scope = match scope_type.as_str() {
        "global" => MemoryScope::Global,
        "project" => MemoryScope::Project(scope_id.unwrap_or_default()),
        "session" => MemoryScope::Session(scope_id.unwrap_or_default()),
        other => anyhow::bail!("Unknown scope_type: {other}"),
    };

    Ok(KnowledgeNode {
        id: Uuid::parse_str(&id_str)?,
        content: r.get("content"),
        tags: serde_json::from_str(&tags_json).unwrap_or_default(),
        verification_path: r.get("verification_path"),
        content_hash: r.get("content_hash"),
        utility_score: r.get("utility_score"),
        scope,
        is_stale: r.get::<i32, _>("is_stale") != 0,
        created_at: created_at_str.parse()?,
        updated_at: updated_at_str.parse()?,
    })
}

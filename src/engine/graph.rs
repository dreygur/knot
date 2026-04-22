use crate::memory::{Edge, EdgeType, KnowledgeNode, MemoryScope};
use crate::skills::SkillNode;
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
                id                TEXT PRIMARY KEY,
                content           TEXT NOT NULL,
                tags              TEXT NOT NULL,
                verification_path TEXT,
                content_hash      TEXT,
                utility_score     REAL NOT NULL DEFAULT 0.5,
                scope_type        TEXT NOT NULL,
                scope_id          TEXT,
                is_stale          INTEGER NOT NULL DEFAULT 0,
                parent_id         TEXT,
                origin_agent      TEXT,
                created_at        TEXT NOT NULL,
                updated_at        TEXT NOT NULL
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        // Additive migrations - safe to run on existing databases.
        let _ = sqlx::query("ALTER TABLE nodes ADD COLUMN parent_id TEXT")
            .execute(&self.pool)
            .await;
        let _ = sqlx::query("ALTER TABLE nodes ADD COLUMN origin_agent TEXT")
            .execute(&self.pool)
            .await;

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

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_nodes_parent ON nodes(parent_id);")
            .execute(&self.pool)
            .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS skills (
                id                   TEXT PRIMARY KEY,
                name                 TEXT NOT NULL UNIQUE,
                description          TEXT NOT NULL,
                prerequisites        TEXT NOT NULL,
                steps                TEXT NOT NULL,
                verification_command TEXT NOT NULL,
                success_count        INTEGER NOT NULL DEFAULT 0,
                utility_score        REAL NOT NULL DEFAULT 0.5,
                related_node_id      TEXT,
                created_at            TEXT NOT NULL,
                updated_at            TEXT NOT NULL
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_skills_name ON skills(name);")
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
                 utility_score, scope_type, scope_id, is_stale,
                 parent_id, origin_agent, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
        .bind(node.parent_id.map(|u| u.to_string()))
        .bind(&node.origin_agent)
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
                is_stale = ?, parent_id = ?, origin_agent = ?, updated_at = ?
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
        .bind(node.parent_id.map(|u| u.to_string()))
        .bind(&node.origin_agent)
        .bind(node.updated_at.to_rfc3339())
        .bind(node.id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_node(&self, id: Uuid) -> Result<Option<KnowledgeNode>> {
        let row = sqlx::query(
            "SELECT id, content, tags, verification_path, content_hash, utility_score, \
             scope_type, scope_id, is_stale, parent_id, origin_agent, created_at, updated_at \
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

    /// Re-parent all direct children of `old_parent` to `new_parent` (or NULL).
    /// Returns the count of rows updated.
    pub async fn reparent_children(
        &self,
        old_parent: Uuid,
        new_parent: Option<Uuid>,
    ) -> Result<usize> {
        let result = sqlx::query("UPDATE nodes SET parent_id = ? WHERE parent_id = ?")
            .bind(new_parent.map(|u| u.to_string()))
            .bind(old_parent.to_string())
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() as usize)
    }

    /// Returns the `success_count` of a skill if it exists.
    pub async fn get_skill_success_count(&self, name: &str) -> Result<Option<i32>> {
        let row = sqlx::query_scalar("SELECT success_count FROM skills WHERE name = ?")
            .bind(name)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row)
    }

    pub async fn delete_skill_by_name(&self, name: &str) -> Result<()> {
        sqlx::query("DELETE FROM skills WHERE name = ?")
            .bind(name)
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
             scope_type, scope_id, is_stale, parent_id, origin_agent, created_at, updated_at \
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

    /// Walk up the parent_id chain and return ancestors from immediate parent to root.
    /// Capped at 16 hops to guard against cycles.
    pub async fn fetch_ancestry(&self, node_id: Uuid) -> Result<Vec<KnowledgeNode>> {
        let mut chain = Vec::new();
        let mut current_id = node_id;
        let mut visited = std::collections::HashSet::new();
        visited.insert(current_id);

        for _ in 0..16 {
            let Some(node) = self.get_node(current_id).await? else {
                break;
            };
            let Some(parent_id) = node.parent_id else {
                break;
            };
            if visited.contains(&parent_id) {
                break; // cycle guard
            }
            visited.insert(parent_id);
            let Some(parent) = self.get_node(parent_id).await? else {
                break;
            };
            current_id = parent.id;
            chain.push(parent);
        }

        Ok(chain)
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

    /// Clear the stale flag - called when Jit-V re-verifies a previously-stale node.
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

    pub async fn insert_skill(&self, skill: &SkillNode) -> Result<()> {
        let prereq_json = serde_json::to_string(&skill.prerequisites)?;
        let steps_json = serde_json::to_string(&skill.steps)?;
        sqlx::query(
            r#"
            INSERT INTO skills
                (id, name, description, prerequisites, steps, verification_command,
                 success_count, utility_score, related_node_id, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(skill.id.to_string())
        .bind(&skill.name)
        .bind(&skill.description)
        .bind(&prereq_json)
        .bind(&steps_json)
        .bind(&skill.verification_command)
        .bind(skill.success_count)
        .bind(skill.utility_score)
        .bind(skill.related_node_id.map(|u: Uuid| u.to_string()))
        .bind(skill.created_at.to_rfc3339())
        .bind(skill.updated_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_skill(&self, name: &str) -> Result<Option<SkillNode>> {
        let row = sqlx::query(
            "SELECT id, name, description, prerequisites, steps, verification_command, \
             success_count, utility_score, related_node_id, created_at, updated_at \
             FROM skills WHERE name = ?",
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;

        row.map(|r| row_to_skill(&r)).transpose()
    }

    pub async fn list_skills(&self) -> Result<Vec<SkillNode>> {
        let rows = sqlx::query(
            "SELECT id, name, description, prerequisites, steps, verification_command, \
             success_count, utility_score, related_node_id, created_at, updated_at \
             FROM skills ORDER BY utility_score DESC",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut skills = Vec::new();
        for r in rows.iter() {
            if let Ok(skill) = row_to_skill(r) {
                skills.push(skill);
            }
        }
        Ok(skills)
    }

    pub async fn increment_skill_success(&self, name: &str) -> Result<()> {
        sqlx::query(
            "UPDATE skills SET success_count = success_count + 1, \
             utility_score = MIN(1.0, utility_score + 0.05), updated_at = ? \
             WHERE name = ?",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(name)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn count_nodes_by_scope(&self) -> Result<(i64, i64, i64)> {
        let l1: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM nodes WHERE scope_type = 'session'")
            .fetch_one(&self.pool)
            .await?;
        let l2: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM nodes WHERE scope_type = 'project'")
            .fetch_one(&self.pool)
            .await?;
        let l3: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM nodes WHERE scope_type = 'global'")
            .fetch_one(&self.pool)
            .await?;
        Ok((l1, l2, l3))
    }

    pub async fn count_skills(&self) -> Result<i64> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM skills")
            .fetch_one(&self.pool)
            .await?;
        Ok(count)
    }

    /// All nodes that have a verification_path (candidates for ghost detection).
    pub async fn list_nodes_with_path(&self) -> Result<Vec<KnowledgeNode>> {
        let rows = sqlx::query(
            "SELECT id, content, tags, verification_path, content_hash, utility_score, \
             scope_type, scope_id, is_stale, parent_id, origin_agent, created_at, updated_at \
             FROM nodes WHERE verification_path IS NOT NULL",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().filter_map(|r| row_to_node(r).ok()).collect())
    }

    pub async fn health_check(&self) -> Result<String> {
        let result = sqlx::query("SELECT 1").fetch_optional(&self.pool).await?;
        match result {
            Some(_) => Ok("ok".into()),
            None => Ok("error".into()),
        }
    }
}

fn row_to_node(r: &sqlx::sqlite::SqliteRow) -> Result<KnowledgeNode> {
    let id_str: String = r.get("id");
    let tags_json: String = r.get("tags");
    let scope_type: String = r.get("scope_type");
    let scope_id: Option<String> = r.get("scope_id");
    let parent_id_str: Option<String> = r.get("parent_id");
    let created_at_str: String = r.get("created_at");
    let updated_at_str: String = r.get("updated_at");

    let scope = match scope_type.as_str() {
        "global" => MemoryScope::Global,
        "project" => MemoryScope::Project(scope_id.unwrap_or_default()),
        "session" => MemoryScope::Session(scope_id.unwrap_or_default()),
        other => anyhow::bail!("Unknown scope_type: {other}"),
    };

    let parent_id = parent_id_str.as_deref().map(Uuid::parse_str).transpose()?;

    Ok(KnowledgeNode {
        id: Uuid::parse_str(&id_str)?,
        content: r.get("content"),
        tags: serde_json::from_str(&tags_json).unwrap_or_default(),
        verification_path: r.get("verification_path"),
        content_hash: r.get("content_hash"),
        utility_score: r.get("utility_score"),
        scope,
        is_stale: r.get::<i32, _>("is_stale") != 0,
        parent_id,
        origin_agent: r.get("origin_agent"),
        created_at: created_at_str.parse()?,
        updated_at: updated_at_str.parse()?,
    })
}

fn row_to_skill(r: &sqlx::sqlite::SqliteRow) -> Result<SkillNode> {
    let id_str: String = r.get("id");
    let prereq_json: String = r.get("prerequisites");
    let steps_json: String = r.get("steps");
    let related_str: Option<String> = r.get("related_node_id");
    let created_at_str: String = r.get("created_at");
    let updated_at_str: String = r.get("updated_at");

    Ok(SkillNode {
        id: Uuid::parse_str(&id_str)?,
        name: r.get("name"),
        description: r.get("description"),
        prerequisites: serde_json::from_str(&prereq_json).unwrap_or_default(),
        steps: serde_json::from_str(&steps_json).unwrap_or_default(),
        verification_command: r.get("verification_command"),
        success_count: r.get("success_count"),
        utility_score: r.get("utility_score"),
        related_node_id: related_str.as_deref().and_then(|s| Uuid::parse_str(s).ok()),
        created_at: created_at_str.parse()?,
        updated_at: updated_at_str.parse()?,
    })
}

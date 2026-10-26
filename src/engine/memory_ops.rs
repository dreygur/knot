use super::{
    CommitReport, DeleteWisdomReport, RecallResult, RejectionReason, RejectionRecord, SaveRequest,
    StatusReport, StorageEngine,
};
use crate::jitv;
use crate::memory::privacy;
use crate::memory::{Edge, EdgeType, KnowledgeNode, MemoryScope, VerificationStatus};
use anyhow::Result;
use uuid::Uuid;

impl StorageEngine {
    /// Persist a new knowledge node.
    ///
    /// Exit-code gate: if `command_exit_code` is `Some(n)` and `n != 0`, the node is
    /// demoted to Session scope regardless of the requested scope. Only successful
    /// commands earn durable memory.
    pub async fn save(&self, req: SaveRequest) -> Result<KnowledgeNode> {
        // Path traversal firewall - reject any verification_path containing ../
        if let Some(ref p) = req.verification_path {
            check_path_traversal(p)?;
        }

        let effective_scope = match req.command_exit_code {
            Some(code) if code != 0 => {
                tracing::info!("exit_code={code} → demoting to Session scope");
                MemoryScope::Session(req.session_id.clone())
            }
            _ => req.scope,
        };

        let clean_content = privacy::scrub(&req.content);
        let content_hash = req.verification_path.as_deref().and_then(jitv::hash_path);

        // Register content → path mapping for future relink operations.
        if let (Some(ref hash), Some(ref path)) = (&content_hash, &req.verification_path) {
            if let Err(e) = self.graph.upsert_path_map(hash, path).await {
                tracing::warn!("path_map upsert failed: {e}");
            }
        }

        let node = KnowledgeNode::new(
            clean_content,
            req.tags,
            req.verification_path,
            content_hash,
            effective_scope,
            req.parent_id,
            req.origin_agent,
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
    /// - If distance > 0.7, confidence is "low" and a [KNOT] WARN is emitted.
    pub async fn recall(&self, query: &str, limit: usize) -> Result<Vec<RecallResult>> {
        let candidates = self.vectors.search(query, limit * 2).await?;

        let mut results = Vec::new();
        for (id, distance) in candidates {
            let Some(mut node) = self.graph.get_node(id).await? else {
                continue;
            };

            // ── Jit-V verification ────────────────────────────────────────────
            let vr = jitv::verify(&node);
            let stale_by_inheritance = if node.parent_id.is_some() {
                self.check_parent_stale(node.parent_id).await?
            } else {
                false
            };

            let mut status = vr.status.clone();
            if stale_by_inheritance {
                status = VerificationStatus::StaleByInheritance;
            }

            // Auto-heal: if the file is missing, scan the project for its hash.
            if matches!(status, VerificationStatus::StaleMissing) {
                if self.relink_stale_wisdom(&mut node).await.unwrap_or(false) {
                    status = VerificationStatus::Verified;
                }
            }

            match status {
                VerificationStatus::Abstract | VerificationStatus::Verified => {
                    if node.is_stale {
                        self.graph.clear_stale(node.id).await?;
                        node.is_stale = false;
                        tracing::info!("Node {} recovered from stale state", node.id);
                    }
                    self.graph.increment_utility(node.id).await?;
                }
                _ => {
                    if !node.is_stale {
                        self.graph.mark_stale(node.id).await?;
                        node.is_stale = true;
                        tracing::info!("Marked node {} stale: {}", node.id, vr.detail);
                    }
                }
            }

            // ── Confidence scoring (semantic drift) ───────────────────────────
            let confidence = if distance > 0.7 {
                eprintln!(
                    "[KNOT] WARN: low confidence for node {} (dist={:.4}) - verify or re-learn",
                    node.id, distance
                );
                "low"
            } else {
                "high"
            };

            // ── Ancestry chain ────────────────────────────────────────────────
            let ancestry = self.graph.fetch_ancestry(node.id).await?;
            let annotated = jitv::annotate(&node, &vr).unwrap_or_else(|| node.content.clone());
            let is_stale = vr.status.is_stale() || stale_by_inheritance;

            results.push(RecallResult {
                node,
                annotated_content: annotated,
                distance,
                confidence: confidence.to_string(),
                is_stale,
                ancestry,
            });
        }

        results.sort_by(|a, b| {
            a.is_stale
                .cmp(&b.is_stale)
                .then(a.distance.partial_cmp(&b.distance).unwrap())
        });
        results.truncate(limit);
        Ok(results)
    }

    async fn check_parent_stale(&self, parent_id: Option<Uuid>) -> Result<bool> {
        let Some(pid) = parent_id else {
            return Ok(false);
        };
        let Some(parent) = self.graph.get_node(pid).await? else {
            return Ok(false);
        };
        Ok(parent.is_stale)
    }

    /// Strict firewall: promote Session-scope nodes to Project scope only if they
    /// pass a fresh Jit-V check at promotion time.
    ///
    /// Every node is accounted for - the returned `CommitReport` records every
    /// promotion and every rejection with a reason. Nothing crosses the boundary
    /// silently.
    pub async fn commit_session(&self, session_id: &str, project_id: &str) -> Result<CommitReport> {
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
                self.graph.mark_stale(node.id).await?;
                node.is_stale = true;

                let reason = match vr.status {
                    VerificationStatus::StaleMissing => RejectionReason::StaleMissing,
                    VerificationStatus::StaleModified => RejectionReason::StaleModified,
                    _ => RejectionReason::PreviouslyStaleConfirmed,
                };

                tracing::warn!(
                    "commit_session BLOCKED node {} reason={} detail={}",
                    node.id,
                    reason.label(),
                    vr.detail
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

            tracing::info!(
                "commit_session PROMOTED node {} → project/{project_id}",
                node.id
            );
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

    /// Delete a knowledge node and re-parent its children to the node's own parent.
    pub async fn delete_wisdom(&self, id: Uuid) -> Result<Option<DeleteWisdomReport>> {
        let Some(node) = self.graph.get_node(id).await? else {
            return Ok(None);
        };
        let children_reparented = self.graph.reparent_children(id, node.parent_id).await?;
        self.graph.delete_node(id).await?;
        self.vectors.delete(id).await?;
        eprintln!(
            "[KNOT] WARN: Memory node {} deleted ({} child(ren) re-parented).",
            id, children_reparented
        );
        Ok(Some(DeleteWisdomReport {
            node_id: id,
            children_reparented,
        }))
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

    /// Ghost nodes: have a verification_path whose file no longer exists.
    pub async fn list_ghost_nodes(&self) -> Result<Vec<KnowledgeNode>> {
        let candidates = self.graph.list_nodes_with_path().await?;
        Ok(candidates
            .into_iter()
            .filter(|n| {
                n.verification_path
                    .as_deref()
                    .map(|p| !std::path::Path::new(p).exists())
                    .unwrap_or(false)
            })
            .collect())
    }

    /// Move all ghost nodes (source file gone and unrelink-able) to archived_nodes.
    /// Auto-deletes archived nodes older than 30 days with success_count = 0.
    /// Returns count archived in this run.
    pub async fn prune_ghosts(&self) -> Result<usize> {
        let ghosts = self.list_ghost_nodes().await?;
        let count = ghosts.len();
        for node in &ghosts {
            self.graph.archive_node(node).await?;
            let _ = self.vectors.delete(node.id).await;
            tracing::info!("[KNOT] Archived ghost node {}", node.id);
        }

        let cutoff = (chrono::Utc::now() - chrono::Duration::days(30)).to_rfc3339();
        let deleted = self.graph.prune_old_archives(&cutoff).await?;
        if deleted > 0 {
            tracing::info!("[KNOT] Auto-purged {deleted} stale archive(s) older than 30 days");
        }

        tracing::info!(
            "[KNOT] prune_ghosts: archived {count} ghost(s), purged {deleted} old archive(s)"
        );
        Ok(count)
    }

    pub async fn knot_status(&self) -> Result<StatusReport> {
        let (l1, l2, l3) = self.graph.count_nodes_by_scope().await?;
        let skills = self.graph.count_skills().await?;
        let db_health = self.graph.health_check().await?;
        let ghost_count = self.list_ghost_nodes().await?.len() as i64;
        let archived_count = self.graph.count_archived_nodes().await?;
        Ok(StatusReport {
            l1_nodes: l1,
            l2_nodes: l2,
            l3_nodes: l3,
            skills,
            db_health,
            ghost_count,
            archived_count,
        })
    }

    /// Attempt to find a stale node's file at a new path by scanning the project tree.
    /// On success, updates verification_path + path_map and clears the stale flag.
    pub async fn relink_stale_wisdom(&self, node: &mut KnowledgeNode) -> Result<bool> {
        let hash = match &node.content_hash {
            Some(h) => h.clone(),
            None => return Ok(false),
        };
        let last_path = match &node.verification_path {
            Some(p) => p.clone(),
            None => return Ok(false),
        };

        let search_root = find_project_root(&last_path);
        let Some(new_path) = scan_for_hash(&search_root, &hash, 8) else {
            return Ok(false);
        };

        let new_hash = jitv::hash_path(&new_path);
        self.graph
            .update_verification_path(node.id, &new_path, new_hash.as_deref())
            .await?;
        self.graph.upsert_path_map(&hash, &new_path).await?;

        node.verification_path = Some(new_path.clone());
        node.content_hash = new_hash;
        node.is_stale = false;
        tracing::info!("[KNOT] Relinked node {} → {}", node.id, new_path);
        Ok(true)
    }
}

/// Walk up from `path`'s parent directory until a `.git` marker is found or
/// we reach the home directory. Returns the project root to use as scan base.
fn find_project_root(path: &str) -> std::path::PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/"));
    let mut current = std::path::Path::new(path)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| home.clone());

    loop {
        if current.join(".git").exists() {
            return current;
        }
        if current == home {
            return home;
        }
        match current.parent() {
            Some(p) if p != current.as_path() => current = p.to_path_buf(),
            _ => return home,
        }
    }
}

/// Recursively scan `root` for a file whose BLAKE3 digest matches `target_hash`.
/// Skips hidden directories, `target/`, and `node_modules/`. Depth-capped at `max`.
fn scan_for_hash(root: &std::path::Path, target_hash: &str, max: usize) -> Option<String> {
    if max == 0 {
        return None;
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return None;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with('.') || matches!(name, "target" | "node_modules") {
                continue;
            }
            if let Some(found) = scan_for_hash(&path, target_hash, max - 1) {
                return Some(found);
            }
        } else if path.is_file() {
            if let Ok(hash) = crate::utils::calculate_hash(&path) {
                if hash == target_hash {
                    return path.to_str().map(String::from);
                }
            }
        }
    }
    None
}

/// Reject paths that attempt directory traversal via `../`.
fn check_path_traversal(path: &str) -> Result<()> {
    let p = std::path::Path::new(path);
    for component in p.components() {
        if component == std::path::Component::ParentDir {
            anyhow::bail!("[KNOT] BLOCKED: directory traversal detected in path '{path}'");
        }
    }
    Ok(())
}

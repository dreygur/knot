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
        // Path traversal firewall — reject any verification_path containing ../
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
    /// Every node is accounted for — the returned `CommitReport` records every
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
        Ok(Some(DeleteWisdomReport { node_id: id, children_reparented }))
    }

    pub async fn list(
        &self,
        scope_type: Option<&str>,
        scope_id: Option<&str>,
        tag_filter: Option<&str>,
    ) -> Result<Vec<KnowledgeNode>> {
        self.graph.list_nodes(scope_type, scope_id, tag_filter).await
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

    /// Delete all ghost nodes (source file gone) from DB and vector store.
    pub async fn prune_ghosts(&self) -> Result<usize> {
        let ghosts = self.list_ghost_nodes().await?;
        let count = ghosts.len();
        for node in &ghosts {
            self.graph.delete_node(node.id).await?;
            let _ = self.vectors.delete(node.id).await;
            tracing::info!("[KNOT] pruned ghost node {}", node.id);
        }
        tracing::info!("[KNOT] prune_ghosts: removed {count} ghost(s)");
        Ok(count)
    }

    pub async fn knot_status(&self) -> Result<StatusReport> {
        let (l1, l2, l3) = self.graph.count_nodes_by_scope().await?;
        let skills = self.graph.count_skills().await?;
        let db_health = self.graph.health_check().await?;
        let ghost_count = self.list_ghost_nodes().await?.len() as i64;
        Ok(StatusReport { l1_nodes: l1, l2_nodes: l2, l3_nodes: l3, skills, db_health, ghost_count })
    }
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

use crate::engine::{
    CommitReport, DeleteSkillResult, RecallResult, SaveRequest, StatusReport, StorageEngine,
};
use crate::memory::{EdgeType, MemoryScope};
use anyhow::Result;
use rmcp::{
    model::{
        CallToolResult, Content, Implementation, InitializeResult, ProtocolVersion,
        ServerCapabilities,
    },
    tool, Error as McpError, ServerHandler,
};
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Clone)]
pub struct KnotServer {
    engine: Arc<Mutex<StorageEngine>>,
    session_id: String,
    /// True when KNOT_READ_ONLY env var is set — write tools return an error.
    read_only: bool,
}

impl KnotServer {
    pub fn new(engine: StorageEngine, session_id: String) -> Self {
        let read_only = std::env::var("KNOT_READ_ONLY").is_ok();
        if read_only {
            eprintln!("[KNOT] WARN:  Vault is locked (Read-Only Mode)");
        }
        Self {
            engine: Arc::new(Mutex::new(engine)),
            session_id,
            read_only,
        }
    }
}

// ── Input schemas ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SaveWisdomInput {
    /// The knowledge to persist (privacy-scrubbed automatically)
    pub content: String,
    /// Descriptive tags (e.g. ["lancedb", "vector-store", "rust"])
    pub tags: Vec<String>,
    /// Optional filesystem path whose existence/hash validates this node
    pub verification_path: Option<String>,
    /// Memory scope: "global", "project", or "session" (default)
    pub scope: Option<String>,
    /// Exit code of the associated CLI command (0 required for L1+ commit)
    pub command_exit_code: Option<i32>,
    /// Project ID when scope is "project"
    pub project_id: Option<String>,
    /// Parent node ID for hierarchical inheritance
    pub parent_id: Option<String>,
    /// Agent identifier that created this node
    pub origin_agent: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RecallMemoryInput {
    /// Natural-language query for semantic search
    pub query: String,
    /// Max results to return (default 5, max 20)
    pub limit: Option<usize>,
    /// Return full content for every result. Default: summary mode when >3 results.
    pub full_content: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct JitVerifyInput {
    /// UUID of the node to verify
    pub node_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CommitSessionInput {
    /// Project ID to promote session nodes into
    pub project_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListNodesInput {
    /// Filter: "global", "project", or "session"
    pub scope_type: Option<String>,
    /// Filter by scope ID (project_id or session_id)
    pub scope_id: Option<String>,
    /// Filter by tag substring
    pub tag_filter: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LinkNodesInput {
    pub source_id: String,
    pub target_id: String,
    /// "depends_on", "contradicts", "refines", or "parent_scope"
    pub edge_type: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ForgetNodeInput {
    pub node_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SaveSkillInput {
    pub name: String,
    pub description: String,
    pub prerequisites: Vec<String>,
    pub steps: Vec<SkillStepInput>,
    pub verification_command: String,
    pub related_node_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SkillStepInput {
    pub description: String,
    pub command: String,
    pub working_dir: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExecuteSkillInput {
    pub skill_name: String,
    pub variables: Option<Vec<VariableInput>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct VariableInput {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RecallSkillsInput {
    pub query: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteWisdomInput {
    /// UUID of the node to permanently delete
    pub node_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteSkillInput {
    /// Exact name of the skill to delete
    pub skill_name: String,
    /// Required when the skill has success_count > 10
    pub force: Option<bool>,
}

// ── Tool implementations ──────────────────────────────────────────────────────

#[tool(tool_box)]
impl KnotServer {
    #[tool(description = "Persist a knowledge node to the memory pool. \
        Committed to L1/L2 only if command_exit_code is 0 or omitted.")]
    async fn save_wisdom(
        &self,
        #[tool(aggr)] input: SaveWisdomInput,
    ) -> Result<CallToolResult, McpError> {
        if self.read_only {
            return Ok(CallToolResult::success(vec![Content::text(
                "[KNOT] WARN:  Vault is locked (Read-Only Mode)",
            )]));
        }
        let engine = self.engine.lock().await;
        let scope = parse_scope(
            input.scope.as_deref(),
            input.project_id.as_deref(),
            &self.session_id,
        );
        let parent_id = input.parent_id.as_ref().and_then(|s| Uuid::parse_str(s).ok());
        let node = engine
            .save(SaveRequest {
                content: input.content,
                tags: input.tags,
                verification_path: input.verification_path,
                scope,
                command_exit_code: input.command_exit_code,
                session_id: self.session_id.clone(),
                parent_id,
                origin_agent: input.origin_agent,
            })
            .await
            .map_err(mcp_err)?;

        let msg = format!(
            "Saved node {}\nscope={}\nutility_score={:.2}\nverification={}\ntags={:?}",
            node.id,
            node.scope.scope_type(),
            node.utility_score,
            node.content_hash
                .as_deref()
                .map(|h| format!("sha256:{}", &h[..8]))
                .unwrap_or_else(|| "none".into()),
            node.tags,
        );
        Ok(CallToolResult::success(vec![Content::text(msg)]))
    }

    #[tool(description = "Semantic search with Jit-V verification. \
        Stale nodes are tagged [STALE:MISSING] or [STALE:MODIFIED]. \
        Returns summaries when >3 results; pass full_content=true for full detail.")]
    async fn recall_memory(
        &self,
        #[tool(aggr)] input: RecallMemoryInput,
    ) -> Result<CallToolResult, McpError> {
        let engine = self.engine.lock().await;
        let limit = input.limit.unwrap_or(5).min(20);
        let results = engine.recall(&input.query, limit).await.map_err(mcp_err)?;

        if results.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "No matching memories found.",
            )]));
        }

        let full = input.full_content.unwrap_or(false);
        let text = if results.len() > 3 && !full {
            format_recall_summary(&results)
        } else {
            format_recall_results(&results)
        };
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    #[tool(description = "Run Jit-V on a specific node. \
        Updates is_stale if verification path changed or is missing.")]
    async fn jit_verify(
        &self,
        #[tool(aggr)] input: JitVerifyInput,
    ) -> Result<CallToolResult, McpError> {
        let id = parse_uuid(&input.node_id)?;
        let engine = self.engine.lock().await;
        match engine.jit_verify_node(id).await.map_err(mcp_err)? {
            None => Ok(CallToolResult::success(vec![Content::text(format!(
                "Node {id} not found"
            ))])),
            Some((node, detail)) => Ok(CallToolResult::success(vec![Content::text(format!(
                "Node {}\n{}\ncontent: {}",
                node.id,
                detail,
                &node.content[..node.content.len().min(120)]
            ))])),
        }
    }

    #[tool(description = "Strict firewall: promote session-scope nodes to a named project. \
        Every node is accounted for — promoted and rejected nodes are both reported. \
        A node is rejected if its verification_path is missing or its content changed on disk.")]
    async fn commit_session(
        &self,
        #[tool(aggr)] input: CommitSessionInput,
    ) -> Result<CallToolResult, McpError> {
        let engine = self.engine.lock().await;
        let report = engine
            .commit_session(&self.session_id, &input.project_id)
            .await
            .map_err(mcp_err)?;
        Ok(CallToolResult::success(vec![Content::text(
            format_commit_report(&report),
        )]))
    }

    #[tool(description = "List knowledge nodes filtered by scope and/or tags, \
        sorted by utility score descending.")]
    async fn list_nodes(
        &self,
        #[tool(aggr)] input: ListNodesInput,
    ) -> Result<CallToolResult, McpError> {
        let engine = self.engine.lock().await;
        let nodes = engine
            .list(
                input.scope_type.as_deref(),
                input.scope_id.as_deref(),
                input.tag_filter.as_deref(),
            )
            .await
            .map_err(mcp_err)?;

        if nodes.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text("No nodes found.")]));
        }
        let output = nodes
            .iter()
            .map(|n| {
                format!(
                    "• [{}] {} (score={:.2}{}) tags={:?}\n  {}",
                    n.scope.scope_type(),
                    n.id,
                    n.utility_score,
                    if n.is_stale { " STALE" } else { "" },
                    n.tags,
                    &n.content[..n.content.len().min(80)]
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    #[tool(description = "Link two nodes with a typed edge: \
        depends_on, contradicts, refines, or parent_scope.")]
    async fn link_nodes(
        &self,
        #[tool(aggr)] input: LinkNodesInput,
    ) -> Result<CallToolResult, McpError> {
        let src = parse_uuid(&input.source_id)?;
        let tgt = parse_uuid(&input.target_id)?;
        let et = EdgeType::from_str(&input.edge_type)
            .ok_or_else(|| McpError::invalid_params(
                format!("Unknown edge_type: {}", input.edge_type),
                None,
            ))?;
        let engine = self.engine.lock().await;
        let edge = engine.link_nodes(src, tgt, et).await.map_err(mcp_err)?;
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Edge {} created: {} --[{}]--> {}",
            edge.id, src, input.edge_type, tgt
        ))]))
    }

    #[tool(description = "Permanently delete a knowledge node and all associated edges.")]
    async fn forget_node(
        &self,
        #[tool(aggr)] input: ForgetNodeInput,
    ) -> Result<CallToolResult, McpError> {
        let id = parse_uuid(&input.node_id)?;
        let engine = self.engine.lock().await;
        engine.forget(id).await.map_err(mcp_err)?;
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Forgotten node {id}"
        ))]))
    }

    #[tool(description = "Health check: node count by level, skill count, and database status.")]
    async fn knot_status(
        &self,
    ) -> Result<CallToolResult, McpError> {
        let engine = self.engine.lock().await;
        let status = engine.knot_status().await.map_err(mcp_err)?;
        Ok(CallToolResult::success(vec![Content::text(format_status_report(&status))]))
    }

    #[tool(description = "Save a reusable skill procedure with prerequisites, steps, and verification command. \
        Use placeholders like {{entity_name}} for reusability.")]
    async fn save_skill(
        &self,
        #[tool(aggr)] input: SaveSkillInput,
    ) -> Result<CallToolResult, McpError> {
        if self.read_only {
            return Ok(CallToolResult::success(vec![Content::text(
                "[KNOT] WARN:  Vault is locked (Read-Only Mode)",
            )]));
        }
        let engine = self.engine.lock().await;
        let steps: Vec<crate::skills::SkillStep> = input
            .steps
            .into_iter()
            .map(|s| crate::skills::SkillStep {
                description: s.description,
                command: s.command,
                working_dir: s.working_dir,
            })
            .collect();
        let related_node_id = input.related_node_id.and_then(|s| Uuid::parse_str(&s).ok());
        let skill = engine
            .save_skill(
                input.name,
                input.description,
                input.prerequisites,
                steps,
                input.verification_command,
                related_node_id,
            )
            .await
            .map_err(mcp_err)?;
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Saved skill '{}' (id={}, score={:.2})",
            skill.name, skill.id, skill.utility_score
        ))]))
    }

    #[tool(description = "Execute a saved skill with variable substitutions. Performs dry-run check first.")]
    async fn execute_skill(
        &self,
        #[tool(aggr)] input: ExecuteSkillInput,
    ) -> Result<CallToolResult, McpError> {
        let engine = self.engine.lock().await;
        let variables: Vec<(String, String)> = input
            .variables
            .unwrap_or_default()
            .into_iter()
            .map(|v| (v.key, v.value))
            .collect();
        let result = engine
            .execute_skill(&input.skill_name, variables)
            .await
            .map_err(mcp_err)?;

        if result.success {
            Ok(CallToolResult::success(vec![Content::text(
                format!(
                    "[KNOT] SUCCESS: Skill '{}' executed.\n{}\nVerification:\n{}",
                    input.skill_name,
                    result.step_results
                        .iter()
                        .enumerate()
                        .map(|(i, r)| format!("  Step {}: {} → {}", i + 1, r.command, if r.success { "OK" } else { "FAILED" }))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    result.verification_output
                        .as_ref()
                        .map(|v| format!("  {}: {}", if v.success { "PASS" } else { "FAIL" }, v.stdout.lines().next().unwrap_or("")))
                        .unwrap_or("  (no verification)".into())
                ),
            )]))
        } else {
            Ok(CallToolResult::success(vec![Content::text(
                format!(
                    "[KNOT] FAIL: Skill '{}' execution failed.\n{}\n{}",
                    input.skill_name,
                    result.detail,
                    result
                        .step_results
                        .iter()
                        .enumerate()
                        .filter_map(|(i, r)| {
                            if !r.success {
                                Some(format!("  Step {} failed: {}", i + 1, r.stderr))
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                ),
            )]))
        }
    }

    #[tool(description = "Search for skills by name or description.")]
    async fn recall_skills(
        &self,
        #[tool(aggr)] input: RecallSkillsInput,
    ) -> Result<CallToolResult, McpError> {
        let engine = self.engine.lock().await;
        let skills = engine.recall_skills(&input.query).await.map_err(mcp_err)?;

        if skills.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text("No skills found.")]));
        }

        let output = skills
            .iter()
            .map(|s| {
                format!(
                    "• {} (score={:.2}, runs={})\n  {}\n  prereqs: {:?}\n  {} steps",
                    s.name,
                    s.utility_score,
                    s.success_count,
                    s.description,
                    s.prerequisites,
                    s.steps.len()
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    #[tool(description = "[DESTRUCTIVE] This tool permanently removes data from the Knot vault. \
        Deletes a knowledge node and re-parents any child nodes to the deleted node's parent. \
        Edges referencing this node are also removed.")]
    async fn delete_wisdom(
        &self,
        #[tool(aggr)] input: DeleteWisdomInput,
    ) -> Result<CallToolResult, McpError> {
        if self.read_only {
            return Ok(CallToolResult::success(vec![Content::text(
                "[KNOT] WARN:  Vault is locked (Read-Only Mode)",
            )]));
        }
        let id = parse_uuid(&input.node_id)?;
        let engine = self.engine.lock().await;
        match engine.delete_wisdom(id).await.map_err(mcp_err)? {
            None => Ok(CallToolResult::success(vec![Content::text(format!(
                "[KNOT] WARN: Node {id} not found — nothing deleted."
            ))])),
            Some(report) => Ok(CallToolResult::success(vec![Content::text(format!(
                "[KNOT] WARN: Memory node {} deleted ({} child(ren) re-parented).",
                report.node_id, report.children_reparented
            ))])),
        }
    }

    #[tool(description = "[DESTRUCTIVE] This tool permanently removes data from the Knot vault. \
        Deletes a named skill. Requires force=true when success_count > 10 to prevent \
        accidental removal of high-utility skills.")]
    async fn delete_skill(
        &self,
        #[tool(aggr)] input: DeleteSkillInput,
    ) -> Result<CallToolResult, McpError> {
        if self.read_only {
            return Ok(CallToolResult::success(vec![Content::text(
                "[KNOT] WARN:  Vault is locked (Read-Only Mode)",
            )]));
        }
        let force = input.force.unwrap_or(false);
        let engine = self.engine.lock().await;
        match engine.delete_skill(&input.skill_name, force).await.map_err(mcp_err)? {
            DeleteSkillResult::Deleted => Ok(CallToolResult::success(vec![Content::text(format!(
                "[KNOT] SUCCESS: Skill '{}' deleted.", input.skill_name
            ))])),
            DeleteSkillResult::NotFound => Ok(CallToolResult::success(vec![Content::text(format!(
                "[KNOT] WARN: Skill '{}' not found.", input.skill_name
            ))])),
            DeleteSkillResult::HighUtilityBlocked { success_count } => {
                Ok(CallToolResult::success(vec![Content::text(format!(
                    "[KNOT] BLOCKED: Skill '{}' has success_count={} (> 10). \
                     Pass force=true to confirm deletion.",
                    input.skill_name, success_count
                ))]))
            }
        }
    }

    #[tool(description = "[DESTRUCTIVE] This tool permanently removes data from the Knot vault. \
        Identifies and deletes Ghost Nodes — memories whose source files no longer exist on disk. \
        Reports count removed.")]
    async fn prune_ghosts(&self) -> Result<CallToolResult, McpError> {
        if self.read_only {
            return Ok(CallToolResult::success(vec![Content::text(
                "[KNOT] WARN:  Vault is locked (Read-Only Mode)",
            )]));
        }
        let engine = self.engine.lock().await;

        // First report what we found, then prune.
        let ghosts = engine.list_ghost_nodes().await.map_err(mcp_err)?;
        if ghosts.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "[KNOT] INFO:  No ghost nodes found — vault is clean.",
            )]));
        }

        let preview = ghosts
            .iter()
            .map(|n| {
                format!(
                    "  • {} path={} tags={:?}",
                    n.id,
                    n.verification_path.as_deref().unwrap_or("?"),
                    n.tags
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        let count = engine.prune_ghosts().await.map_err(mcp_err)?;
        Ok(CallToolResult::success(vec![Content::text(format!(
            "[KNOT] INFO:  Pruned {count} ghost node(s):\n{preview}"
        ))]))
    }
}

#[tool(tool_box)]
impl ServerHandler for KnotServer {
    fn get_info(&self) -> rmcp::model::ServerInfo {
        InitializeResult {
            protocol_version: ProtocolVersion::default(),
            capabilities: ServerCapabilities {
                tools: Some(Default::default()),
                ..Default::default()
            },
            server_info: Implementation {
                name: "knot".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
            instructions: Some(
                "Knot: persistent memory pool MCP server. \
                save_wisdom → persist knowledge; recall_memory → semantic search + Jit-V; \
                commit_session → promote session learnings to project scope."
                    .into(),
            ),
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn parse_scope(scope_str: Option<&str>, project_id: Option<&str>, session_id: &str) -> MemoryScope {
    match scope_str {
        Some("global") => MemoryScope::Global,
        Some(s) if s.starts_with("project:") => {
            MemoryScope::Project(s.trim_start_matches("project:").to_string())
        }
        Some("project") => MemoryScope::Project(project_id.unwrap_or("default").to_string()),
        _ => MemoryScope::Session(session_id.to_string()),
    }
}

fn format_commit_report(r: &CommitReport) -> String {
    let mut out = format!(
        "SESSION COMMIT REPORT\n\
         session   : {}\n\
         project   : {}\n\
         promoted  : {}\n\
         rejected  : {}\n",
        r.session_id,
        r.project_id,
        r.promoted_count(),
        r.rejected_count(),
    );

    if !r.promoted.is_empty() {
        out.push('\n');
        for id in &r.promoted {
            out.push_str(&format!("  ✓  {id}\n"));
        }
    }

    if !r.rejected.is_empty() {
        out.push('\n');
        for rec in &r.rejected {
            out.push_str(&format!(
                "  ✗  {} [{}]\n     preview : {}\n     reason  : {}\n",
                rec.node_id,
                rec.reason.label(),
                rec.content_preview,
                rec.detail,
            ));
        }
    }

    if r.rejected_count() == 0 {
        out.push_str("\nAll session knowledge passed integrity checks.\n");
    } else {
        out.push_str(&format!(
            "\n{} node(s) blocked — verification paths changed or missing on disk.\n",
            r.rejected_count()
        ));
    }

    out
}

fn format_recall_results(results: &[RecallResult]) -> String {
    results
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let mut out = format!(
                "{}. [{}] {} (score={:.2}, dist={:.4}, confidence={}{})\n   {}",
                i + 1,
                r.node.scope.scope_type(),
                r.node.id,
                r.node.utility_score,
                r.distance,
                r.confidence,
                if r.is_stale { " STALE" } else { "" },
                r.annotated_content.lines().next().unwrap_or("")
            );
            if !r.ancestry.is_empty() {
                out.push_str("\n   ancestry: ");
                out.push_str(&r.ancestry
                    .iter()
                    .map(|a| format!("{}", a.id))
                    .collect::<Vec<_>>()
                    .join(" → "));
            }
            out
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn parse_uuid(s: &str) -> Result<Uuid, McpError> {
    Uuid::parse_str(s).map_err(|e| McpError::invalid_params(e.to_string(), None))
}

fn mcp_err(e: anyhow::Error) -> McpError {
    McpError::internal_error(e.to_string(), None)
}

fn format_recall_summary(results: &[RecallResult]) -> String {
    let mut out = format!(
        "Found {} memories (summary mode — pass full_content=true for details)\n\n",
        results.len()
    );
    for (i, r) in results.iter().enumerate() {
        let stale = if r.is_stale { " [STALE]" } else { "" };
        let snippet: String = r.annotated_content.chars().take(100).collect();
        out.push_str(&format!(
            "{}. [{}] {} (score={:.2}, dist={:.4}{})\n   {}\n\n",
            i + 1,
            r.node.scope.scope_type(),
            r.node.id,
            r.node.utility_score,
            r.distance,
            stale,
            snippet
        ));
    }
    out
}

fn format_status_report(r: &StatusReport) -> String {
    let ghost_line = if r.ghost_count > 0 {
        format!("│ Ghosts       : {:>4}  ← run prune_ghosts\n", r.ghost_count)
    } else {
        format!("│ Ghosts       : {:>4}\n", r.ghost_count)
    };
    format!(
        "KNOT STATUS\n\
         ──────────\n\
         │ L1 (Session)  : {:>4}\n\
         │ L2 (Project)  : {:>4}\n\
         │ L3 (Global)   : {:>4}\n\
         │ Skills        : {:>4}\n\
         {}\
         │ DB Health     : {}\n\
         ──────────",
        r.l1_nodes,
        r.l2_nodes,
        r.l3_nodes,
        r.skills,
        ghost_line,
        r.db_health
    )
}

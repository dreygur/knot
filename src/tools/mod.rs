use crate::engine::{CommitReport, RecallResult, StorageEngine};
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
}

impl KnotServer {
    pub fn new(engine: StorageEngine, session_id: String) -> Self {
        Self {
            engine: Arc::new(Mutex::new(engine)),
            session_id,
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
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RecallMemoryInput {
    /// Natural-language query for semantic search
    pub query: String,
    /// Max results to return (default 5, max 20)
    pub limit: Option<usize>,
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

// ── Tool implementations ──────────────────────────────────────────────────────

#[tool(tool_box)]
impl KnotServer {
    #[tool(description = "Persist a knowledge node to the memory pool. \
        Committed to L1/L2 only if command_exit_code is 0 or omitted.")]
    async fn save_wisdom(
        &self,
        #[tool(aggr)] input: SaveWisdomInput,
    ) -> Result<CallToolResult, McpError> {
        let engine = self.engine.lock().await;
        let scope = parse_scope(
            input.scope.as_deref(),
            input.project_id.as_deref(),
            &self.session_id,
        );
        let node = engine
            .save(
                input.content,
                input.tags,
                input.verification_path,
                scope,
                input.command_exit_code,
                &self.session_id,
            )
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
        Stale nodes are tagged [STALE:MISSING] or [STALE:MODIFIED].")]
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
        Ok(CallToolResult::success(vec![Content::text(
            format_recall_results(&results),
        )]))
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
}

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
            format!(
                "{}. [{}] {} (score={:.2}, dist={:.4}{})\n   {}",
                i + 1,
                r.node.scope.scope_type(),
                r.node.id,
                r.node.utility_score,
                r.distance,
                if r.is_stale { " STALE" } else { "" },
                r.annotated_content.lines().next().unwrap_or("")
            )
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

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The three-tier memory hierarchy.
/// Promotion path: Session → Project (exit_code=0) → Global (utility_score ≥ 0.8)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "id")]
pub enum MemoryScope {
    Global,
    Project(String),
    Session(String),
}

impl MemoryScope {
    pub fn scope_type(&self) -> &'static str {
        match self {
            MemoryScope::Global => "global",
            MemoryScope::Project(_) => "project",
            MemoryScope::Session(_) => "session",
        }
    }

    pub fn scope_id(&self) -> Option<&str> {
        match self {
            MemoryScope::Global => None,
            MemoryScope::Project(id) | MemoryScope::Session(id) => Some(id.as_str()),
        }
    }
}

/// Typed relationships between knowledge nodes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EdgeType {
    /// A depends on B to be valid
    DependsOn,
    /// A and B cannot both be true
    Contradicts,
    /// A is a more specific/updated version of B
    Refines,
    /// A is a child scope of B (Session→Project, Project→Global)
    ParentScope,
}

impl EdgeType {
    pub fn as_str(&self) -> &'static str {
        match self {
            EdgeType::DependsOn => "depends_on",
            EdgeType::Contradicts => "contradicts",
            EdgeType::Refines => "refines",
            EdgeType::ParentScope => "parent_scope",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "depends_on" => Some(EdgeType::DependsOn),
            "contradicts" => Some(EdgeType::Contradicts),
            "refines" => Some(EdgeType::Refines),
            "parent_scope" => Some(EdgeType::ParentScope),
            _ => None,
        }
    }
}

/// A single unit of verified knowledge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeNode {
    pub id: Uuid,
    /// Privacy-scrubbed content
    pub content: String,
    pub tags: Vec<String>,
    /// Filesystem path whose existence/hash validates this node
    pub verification_path: Option<String>,
    /// BLAKE3 hex digest of verification_path content at save-time
    pub content_hash: Option<String>,
    /// 0.0–1.0; incremented on each recall hit
    pub utility_score: f32,
    pub scope: MemoryScope,
    /// True if Jit-V detected path missing or hash mismatch
    pub is_stale: bool,
    /// Optional parent node for hierarchical knowledge inheritance
    pub parent_id: Option<Uuid>,
    /// Agent identifier that created this node — enforces namespace protection
    pub origin_agent: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl KnowledgeNode {
    pub fn new(
        content: String,
        tags: Vec<String>,
        verification_path: Option<String>,
        content_hash: Option<String>,
        scope: MemoryScope,
        parent_id: Option<Uuid>,
        origin_agent: Option<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            content,
            tags,
            verification_path,
            content_hash,
            utility_score: 0.5,
            scope,
            is_stale: false,
            parent_id,
            origin_agent,
            created_at: now,
            updated_at: now,
        }
    }

    /// Increment utility score on recall hit, capped at 1.0
    pub fn hit(&mut self) {
        self.utility_score = (self.utility_score + 0.05).min(1.0);
        self.updated_at = Utc::now();
    }

    /// Qualifies for Global promotion
    pub fn is_promotion_candidate(&self) -> bool {
        self.utility_score >= 0.8 && !self.is_stale
    }
}

/// A directed, typed edge between two KnowledgeNodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub id: Uuid,
    pub source_id: Uuid,
    pub target_id: Uuid,
    pub edge_type: EdgeType,
    pub created_at: DateTime<Utc>,
}

impl Edge {
    pub fn new(source_id: Uuid, target_id: Uuid, edge_type: EdgeType) -> Self {
        Self {
            id: Uuid::new_v4(),
            source_id,
            target_id,
            edge_type,
            created_at: Utc::now(),
        }
    }
}

/// Jit-V result for a single node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationResult {
    pub node_id: Uuid,
    pub status: VerificationStatus,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum VerificationStatus {
    /// No verification_path — abstract knowledge, always valid
    Abstract,
    /// Path exists and hash matches
    Verified,
    /// Path does not exist
    StaleMissing,
    /// Path exists but content changed
    StaleModified,
    /// Node is locally clean but a parent node is stale
    StaleByInheritance,
}

impl VerificationStatus {
    pub fn tag(&self) -> &'static str {
        match self {
            VerificationStatus::Abstract => "",
            VerificationStatus::Verified => "",
            VerificationStatus::StaleMissing => "[STALE:MISSING]",
            VerificationStatus::StaleModified => "[STALE:MODIFIED]",
            VerificationStatus::StaleByInheritance => "[STALE:INHERITED]",
        }
    }

    pub fn is_stale(&self) -> bool {
        matches!(
            self,
            VerificationStatus::StaleMissing
                | VerificationStatus::StaleModified
                | VerificationStatus::StaleByInheritance
        )
    }
}

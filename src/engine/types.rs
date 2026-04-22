use crate::memory::KnowledgeNode;
use uuid::Uuid;

/// Result of a single recall hit — includes Jit-V annotation and distance.
#[derive(Debug)]
pub struct RecallResult {
    pub node: KnowledgeNode,
    /// Content with stale tag prepended if applicable.
    pub annotated_content: String,
    pub distance: f32,
    pub confidence: String,
    pub is_stale: bool,
    pub ancestry: Vec<KnowledgeNode>,
}

#[derive(Debug)]
pub struct StepResult {
    pub command: String,
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug)]
pub struct VerificationOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug)]
pub struct ExecutionResult {
    pub success: bool,
    pub step_results: Vec<StepResult>,
    pub verification_output: Option<VerificationOutput>,
    pub detail: String,
}

/// Parameters for persisting a new knowledge node.
pub struct SaveRequest {
    pub content: String,
    pub tags: Vec<String>,
    pub verification_path: Option<String>,
    pub scope: crate::memory::MemoryScope,
    pub command_exit_code: Option<i32>,
    pub session_id: String,
    pub parent_id: Option<Uuid>,
    pub origin_agent: Option<String>,
}

pub struct DeleteWisdomReport {
    pub node_id: Uuid,
    pub children_reparented: usize,
}

pub enum DeleteSkillResult {
    Deleted,
    NotFound,
    /// Skill has been used successfully more than 10 times; force=true required.
    HighUtilityBlocked {
        success_count: i32,
    },
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
    pub content_preview: String,
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

#[derive(Debug)]
pub struct StatusReport {
    pub l1_nodes: i64,
    pub l2_nodes: i64,
    pub l3_nodes: i64,
    pub skills: i64,
    pub db_health: String,
    /// Nodes whose verification_path no longer exists on disk.
    pub ghost_count: i64,
}

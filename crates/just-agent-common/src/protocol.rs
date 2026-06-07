//! HTTP/SSE wire types for daemon-client communication.

use serde::{Deserialize, Serialize};

use crate::agentid::AgentId;
use crate::approval::{ApprovalStatus, ToolCallContent};
use crate::policy::ToolPolicy;
use crate::promote::{SkillPromoteRecord, SkillPromoteStatus};

/// Agent lifecycle state exposed via the status endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    Idle,
    Busy,
}

impl AgentState {
    pub const IDLE: u8 = 0;
    pub const BUSY: u8 = 1;
}

impl std::fmt::Display for AgentState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            AgentState::Idle => "idle",
            AgentState::Busy => "busy",
        })
    }
}

/// Wire-format event for SSE transport (daemon to client).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SseEvent {
    Reasoning {
        content: String,
    },
    AssistantContent {
        content: String,
    },
    AssistantContentDelta {
        delta: String,
    },
    ReasoningDelta {
        delta: String,
    },
    ToolCall {
        name: String,
        args: String,
    },
    ToolResult {
        result: String,
    },
    Finished {
        content: String,
    },
    MaxRoundsExceeded,
    Error {
        message: String,
    },
    Status {
        message: String,
    },
    Busy,
    ApprovalUpdated {
        id: String,
        status: ApprovalStatus,
    },
    Retrying {
        attempt: u32,
        max_attempts: u32,
        error: String,
        delay_secs: f64,
    },
    Cancelled,
    TokenBudgetExceeded {
        consumed: u64,
        budget: u64,
    },
}

/// Default token budget when none is specified (100M tokens).
pub const DEFAULT_TOKEN_BUDGET: u64 = 100_000_000;

/// Request body for creating a new agent instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateAgentRequest {
    pub workspace_root: Option<String>,
    pub skills: Vec<String>,
    pub prompt: Option<String>,
    pub created_by: Option<AgentId>,
}

/// Response body returned after creating an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateAgentResponse {
    pub id: AgentId,
}

/// A single approval entry in API responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalEntry {
    pub id: String,
    pub requested_by: AgentId,
    pub content: ToolCallContent,
    /// Agent-provided justification for the tool call.
    pub commit_reason: Option<String>,
    pub status: ApprovalStatus,
    pub deny_reason: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
}

impl ApprovalEntry {
    /// Construct an [`ApprovalEntry`] from an approval info snapshot and the owning agent id.
    ///
    /// Encapsulates the field-by-field mapping so callers don't need to
    /// repeat the construction at every call site.
    pub fn from_info(
        id: String,
        requested_by: AgentId,
        content: ToolCallContent,
        commit_reason: Option<String>,
        status: ApprovalStatus,
        deny_reason: Option<String>,
        created_at: time::OffsetDateTime,
    ) -> Self {
        Self {
            id,
            requested_by,
            content,
            commit_reason,
            status,
            deny_reason,
            created_at,
        }
    }
}

/// Response for listing approvals.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListApprovalsResponse {
    pub items: Vec<ApprovalEntry>,
    pub total: usize,
}

/// Request body for approving or denying an approval.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalDecisionBody {
    pub decision: String,
    pub reason: Option<String>,
}

/// Response for GET /agents/{id}/permissions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentPermissionsResponse {
    pub max_depth: u8,
    pub workspace_root: String,
    pub created_by: Option<AgentId>,
    pub tool_policy: ToolPolicy,
}

/// Summary of an agent instance returned in list responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSummary {
    pub id: AgentId,
    pub workspace_root: String,
    pub state: AgentState,
    pub created_by: Option<AgentId>,
}

/// Combined agent status: lifecycle state + context usage + recent retry history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentStatusResponse {
    pub state: AgentState,
    pub context: crate::context::ContextUsage,
    pub recent_retries: Vec<crate::retry::RetryRecord>,
    /// Daemon-wide token consumption budget (shared by all agents).
    pub token_budget: u64,
    /// Cumulative daemon-wide tokens consumed toward the budget.
    pub token_consumed: u64,
}

/// Request body for sending a message to an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRequest {
    pub text: String,
}

/// Response body for sending a message to an agent.
///
/// Includes queue depth feedback so callers can gauge expected latency:
/// - `queue_depth == 0`: agent will process the message immediately.
/// - `queue_depth > 0`: message is queued behind existing messages; a
///   warning is included.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageResponse {
    /// Approximate number of messages queued ahead of this one (0 = immediate processing).
    pub queue_depth: usize,
    /// Human-readable note when queue is non-empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

/// Response body for listing agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListAgentsResponse {
    pub agents: Vec<AgentSummary>,
}

/// Query parameters for listing approvals.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ListApprovalsQuery {
    pub offset: Option<u64>,
    /// Page size. Server clamps to [1, 20]; defaults to 5 when unset.
    pub limit: Option<u64>,
    pub requested_by: Option<AgentId>,
    pub status: Option<String>,
    pub order: Option<String>,
}

/// Response for GET /agents/{id}/skills/paths.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillPathsResponse {
    /// Absolute path to the shared skill directory.
    pub shared: String,
    /// Absolute path to the agent-local skill directory, if available.
    pub local: Option<String>,
}

/// Skill metadata parsed from YAML frontmatter.
///
/// Also used as the response for GET /agents/{id}/skills/{name}/meta.
///
/// **Note:** `name` here is a display label from the frontmatter, not the
/// canonical skill identifier. The skill's unique identity is its path
/// relative to the skills root (e.g. `code/refactoring`), which determines
/// the on-disk location and is used for all lookups and routing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMeta {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

// ---------------------------------------------------------------------------
// Skill promote request wire types (review-based promote flow)
// ---------------------------------------------------------------------------

/// Response for POST /agents/{id}/skills/{name}/promote-request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillPromoteSubmitResponse {
    pub request_id: String,
    pub skill_name: String,
    pub status: SkillPromoteStatus,
    /// Whether a shared skill already existed (old content was snapshotted).
    pub has_existing: bool,
}

/// Decision body for POST /skill-promote-requests/{id}.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillPromoteDecisionBody {
    pub decision: PromoteDecision,
    pub reason: Option<String>,
}

/// Decision variants for promote-request review.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromoteDecision {
    Approve,
    Deny,
}

/// A promote request entry in list/get API responses.
/// Does NOT include content — use the show endpoint for that.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillPromoteRecordEntry {
    pub id: String,
    pub skill_name: String,
    /// Whether a shared skill already existed at submission time.
    pub has_existing: bool,
    pub requested_by: AgentId,
    pub status: SkillPromoteStatus,
    pub deny_reason: Option<String>,
    /// Skill description from frontmatter, for reviewer context.
    pub description: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub reviewed_at: Option<time::OffsetDateTime>,
}

impl SkillPromoteRecordEntry {
    /// Construct from a stored [`SkillPromoteRecord`], omitting content fields.
    pub fn from_record(r: &SkillPromoteRecord) -> Self {
        Self {
            id: r.id.clone(),
            skill_name: r.skill_name.clone(),
            has_existing: r.has_existing,
            requested_by: r.requested_by.clone(),
            status: r.status,
            deny_reason: r.deny_reason.clone(),
            description: r.description.clone(),
            created_at: r.created_at,
            reviewed_at: r.reviewed_at,
        }
    }
}

/// Response for GET /skill-promote-requests/{id} (show endpoint).
/// Includes old/new content for diff review.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillPromoteShowResponse {
    pub id: String,
    pub skill_name: String,
    /// Whether a shared skill already existed at submission time.
    pub has_existing: bool,
    pub requested_by: AgentId,
    pub status: SkillPromoteStatus,
    pub deny_reason: Option<String>,
    /// Skill description from frontmatter.
    pub description: Option<String>,
    pub old_content: Option<String>,
    pub new_content: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub reviewed_at: Option<time::OffsetDateTime>,
}

impl SkillPromoteShowResponse {
    /// Construct from a stored [`SkillPromoteRecord`], including content fields.
    pub fn from_record(r: &SkillPromoteRecord) -> Self {
        Self {
            id: r.id.clone(),
            skill_name: r.skill_name.clone(),
            has_existing: r.has_existing,
            requested_by: r.requested_by.clone(),
            status: r.status,
            deny_reason: r.deny_reason.clone(),
            description: r.description.clone(),
            old_content: r.old_content.clone(),
            new_content: r.new_content.clone(),
            created_at: r.created_at,
            reviewed_at: r.reviewed_at,
        }
    }
}

/// Response for GET /skill-promote-requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListSkillPromoteRecordsResponse {
    pub items: Vec<SkillPromoteRecordEntry>,
    pub total: usize,
}

// ---------------------------------------------------------------------------
// Token budget wire types
// ---------------------------------------------------------------------------

/// Request body for POST /budget.
///
/// Exactly one of `set_remaining` or `delta` must be provided.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenBudgetUpdateRequest {
    /// Set remaining budget to this value. The daemon computes the new total
    /// as `consumed + value`. Mutually exclusive with `delta`.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "set_remaining"
    )]
    pub set_remaining: Option<u64>,
    /// Adjust total budget by this signed delta. Mutually exclusive with `set_remaining`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta: Option<i64>,
}

/// Response body for GET/POST /budget.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenBudgetResponse {
    /// Current budget (total token limit).
    pub budget: u64,
    /// Cumulative tokens consumed so far.
    pub consumed: u64,
    /// Remaining tokens before budget exhaustion.
    pub remaining: u64,
}

impl TokenBudgetResponse {
    /// Format this response as a human-readable status string.
    ///
    /// Output example: `"budget: 100.0M  consumed: 23.5M  remaining: 76.5M"`
    pub fn format_display(&self) -> String {
        format!(
            "budget: {}  consumed: {}  remaining: {}",
            crate::tokens::format_tokens_m(self.budget),
            crate::tokens::format_tokens_m(self.consumed),
            crate::tokens::format_tokens_m(self.remaining),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_budget_response_format_display() {
        let resp = TokenBudgetResponse {
            budget: 100_000_000,
            consumed: 23_500_000,
            remaining: 76_500_000,
        };
        assert_eq!(
            resp.format_display(),
            "budget: 100.0M  consumed: 23.5M  remaining: 76.5M",
        );
    }
}

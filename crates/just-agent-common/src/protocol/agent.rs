//! Agent lifecycle and messaging wire types.

use serde::{Deserialize, Serialize};

use crate::agentid::AgentId;
use crate::context::ContextUsage;
use crate::policy::ToolPolicy;
use crate::retry::RetryRecord;

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

/// Round limit for an agent, set via `CreateAgentRequest::max_tool_rounds`.
///
/// - `None` on the request → use daemon default (`JUST_AGENT_MAX_TOOL_ROUNDS` env var
///   or built-in unlimited).
/// - `Some(Unlimited)` → force no round limit (bounded only by token budget).
/// - `Some(Limited(N))` → explicit round limit (must be > 0).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaxToolRounds {
    /// No hard round limit — bounded only by the daemon-wide token budget.
    Unlimited,
    /// Explicit round limit. Must be greater than zero.
    Limited(usize),
}

/// Request body for creating a new agent instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateAgentRequest {
    pub workspace_root: Option<String>,
    pub skills: Vec<String>,
    pub prompt: Option<String>,
    pub created_by: Option<AgentId>,
    /// Override the default/env-configured max tool-call rounds for this agent.
    ///
    /// - `None` → use daemon default (`JUST_AGENT_MAX_TOOL_ROUNDS` or unlimited).
    /// - `Some(MaxToolRounds::Unlimited)` → force unlimited rounds.
    /// - `Some(MaxToolRounds::Limited(N))` → explicit limit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tool_rounds: Option<MaxToolRounds>,
}

/// Response body returned after creating an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateAgentResponse {
    pub id: AgentId,
}

/// Summary of an agent instance returned in list responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSummary {
    pub id: AgentId,
    pub workspace_root: String,
    pub state: AgentState,
    pub created_by: Option<AgentId>,
}

/// Response body for listing agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListAgentsResponse {
    pub agents: Vec<AgentSummary>,
}

/// Combined agent status: lifecycle state + context usage + recent retry history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentStatusResponse {
    pub state: AgentState,
    pub context: ContextUsage,
    pub recent_retries: Vec<RetryRecord>,
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

/// Response for GET /agents/{id}/permissions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentPermissionsResponse {
    pub max_depth: u8,
    pub workspace_root: String,
    pub created_by: Option<AgentId>,
    pub tool_policy: ToolPolicy,
}

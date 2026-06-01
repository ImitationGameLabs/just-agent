use just_agent_common::context::ContextUsage;
use just_agent_common::retry::RetryRecord;
use just_agent_common::types::AgentId;
use just_agent_common::types::AgentState;
pub(crate) use just_agent_common::types::{CreateAgentRequest, CreateAgentResponse};
pub use just_agent_common::types::{
    DeferredActionDecisionBody, DeferredActionEntry, ListDeferredActionsResponse,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
pub(crate) struct MessageRequest {
    pub text: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ListAgentsResponse {
    pub agents: Vec<AgentSummary>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AgentSummary {
    pub id: AgentId,
    pub workspace_root: String,
    pub state: AgentState,
    pub created_by: Option<AgentId>,
}

/// Deferred action info extracted from an SSE `DeferredCreated` event.
#[derive(Debug, Clone)]
pub struct DeferredInfo {
    pub id: String,
    pub tool_name: String,
    pub arguments: serde_json::Value,
    pub reason: String,
    pub dangerous: bool,
}

/// Combined agent status: context usage + retry history.
#[derive(Debug, Deserialize)]
pub struct AgentStatusResponse {
    pub state: AgentState,
    pub context: ContextUsage,
    pub recent_retries: Vec<RetryRecord>,
}

/// Query parameters for listing deferred actions.
#[derive(Debug, Default, Serialize)]
pub struct ListDeferredActionsParams {
    pub offset: Option<u64>,
    /// Page size. Server clamps to [1, 20]; defaults to 5 when unset.
    pub limit: Option<u64>,
    pub requested_by: Option<String>,
    pub status: Option<String>,
    pub order: Option<String>,
}

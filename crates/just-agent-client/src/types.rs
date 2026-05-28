use just_agent_common::context::ContextUsage;
use just_agent_common::retry::RetryRecord;
use just_agent_common::types::AgentId;
use just_agent_common::types::AgentState;
pub(crate) use just_agent_common::types::{CreateAgentRequest, CreateAgentResponse};
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

#[derive(Debug, Serialize)]
pub(crate) struct ApprovalRequestBody {
    pub request_id: String,
    pub decision: String,
    pub reason: Option<String>,
}

/// Deferred action info extracted from an SSE `DeferredCreated` event.
#[derive(Debug, Clone)]
pub struct DeferredInfo {
    pub request_id: String,
    pub tool_name: String,
    pub summary: String,
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

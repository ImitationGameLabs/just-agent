pub mod client;
pub mod types;

pub use client::DaemonClient;
pub use just_agent_common::types::{
    AgentPermissionsResponse, AgentStatusResponse, AgentSummary, ApprovalDecisionBody,
    ApprovalEntry, ApprovalStatus, CreateAgentRequest, CreateAgentResponse, ListAgentsResponse,
    ListApprovalsQuery, ListApprovalsResponse, PolicyDecision, ToolCallContent, ToolPolicy,
};
pub use types::ListApprovalsParams;

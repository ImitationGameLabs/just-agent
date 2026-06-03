pub mod client;
pub mod types;

pub use client::DaemonClient;
pub use just_agent_common::types::{
    AgentPermissionsResponse, ApprovalDecisionBody, ApprovalEntry,
    ApprovalStatus, ListApprovalsResponse, PolicyDecision, ToolCallContent, ToolPolicy,
};
pub use types::{AgentSummary, ListApprovalsParams};

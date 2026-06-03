//! Shared types used across the agent crate.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

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

/// Unique identifier for an agent instance.
///
/// Thin wrapper around a UUID string — provides type safety without format validation.
/// Use [`AgentId::random()`] to generate a new one.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentId(String);

impl AgentId {
    pub fn random() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
}

impl std::fmt::Display for AgentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl AsRef<str> for AgentId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<String> for AgentId {
    fn from(s: String) -> Self {
        AgentId(s)
    }
}

impl From<AgentId> for String {
    fn from(id: AgentId) -> Self {
        id.0
    }
}

impl std::str::FromStr for AgentId {
    type Err = std::convert::Infallible;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(AgentId(s.to_owned()))
    }
}

impl std::borrow::Borrow<str> for AgentId {
    fn borrow(&self) -> &str {
        &self.0
    }
}

#[derive(Debug)]
pub enum AgentEvent {
    Reasoning(String),
    AssistantContent(String),
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
    ToolResult(String),
    Finished(String),
    MaxRoundsExceeded,
    Error(String),
    Status(String),
    Busy,
    ApprovalCommitted {
        id: String,
        tool_name: String,
        arguments: serde_json::Value,
        commit_reason: String,
    },
    Retrying {
        attempt: u32,
        max_attempts: u32,
        error: String,
        delay_secs: f64,
    },
    ApprovalRedeemed {
        id: String,
    },
    ApprovalCancelled {
        id: String,
    },
    Cancelled,
}

/// Outcome of running the agent round loop.
pub enum AgentOutcome {
    Finished { content: String },
    MaxRoundsExceeded,
    Cancelled,
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
}

impl SseEvent {
    /// Convert an AgentEvent to an SSE event for broadcast.
    /// Returns `None` for events handled by other means (e.g., routed to superiors).
    pub fn try_from_agent(event: AgentEvent) -> Option<Self> {
        match event {
            AgentEvent::ApprovalCommitted { .. } => None,
            AgentEvent::ApprovalRedeemed { id } => Some(Self::ApprovalUpdated {
                id,
                status: ApprovalStatus::Redeemed,
            }),
            AgentEvent::ApprovalCancelled { id } => Some(Self::ApprovalUpdated {
                id,
                status: ApprovalStatus::Cancelled,
            }),
            AgentEvent::Reasoning(content) => Some(Self::Reasoning { content }),
            AgentEvent::AssistantContent(content) => Some(Self::AssistantContent { content }),
            AgentEvent::AssistantContentDelta { delta } => {
                Some(Self::AssistantContentDelta { delta })
            }
            AgentEvent::ReasoningDelta { delta } => Some(Self::ReasoningDelta { delta }),
            AgentEvent::ToolCall { name, args } => Some(Self::ToolCall { name, args }),
            AgentEvent::ToolResult(result) => Some(Self::ToolResult { result }),
            AgentEvent::Finished(content) => Some(Self::Finished { content }),
            AgentEvent::MaxRoundsExceeded => Some(Self::MaxRoundsExceeded),
            AgentEvent::Error(msg) => Some(Self::Error { message: msg }),
            AgentEvent::Status(msg) => Some(Self::Status { message: msg }),
            AgentEvent::Busy => Some(Self::Busy),
            AgentEvent::Retrying {
                attempt,
                max_attempts,
                error,
                delay_secs,
            } => Some(Self::Retrying {
                attempt,
                max_attempts,
                error,
                delay_secs,
            }),
            AgentEvent::Cancelled => Some(Self::Cancelled),
        }
    }
}

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

/// Status of an approval request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    Pending,
    Committed,
    Approved,
    Denied,
    Redeemed,
    Cancelled,
}

impl ApprovalStatus {
    /// Parse a status string (e.g. from a query parameter).
    pub fn from_str_name(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "committed" => Some(Self::Committed),
            "approved" => Some(Self::Approved),
            "denied" => Some(Self::Denied),
            "redeemed" => Some(Self::Redeemed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Committed => "committed",
            Self::Approved => "approved",
            Self::Denied => "denied",
            Self::Redeemed => "redeemed",
            Self::Cancelled => "cancelled",
        }
    }
}

impl std::fmt::Display for ApprovalStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Complete tool call content for an approval.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCallContent {
    pub tool_name: String,
    pub arguments: serde_json::Value,
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

/// Decision for a tool in the policy.
///
/// Ordering (via derived `Ord`): Allow < Classify < Ask < Deny.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyDecision {
    Allow,
    Classify,
    Ask,
    Deny,
}

impl std::fmt::Display for PolicyDecision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Allow => "allow",
            Self::Classify => "classify",
            Self::Ask => "ask",
            Self::Deny => "deny",
        })
    }
}

impl std::str::FromStr for PolicyDecision {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "allow" => Ok(Self::Allow),
            "classify" => Ok(Self::Classify),
            "ask" => Ok(Self::Ask),
            "deny" => Ok(Self::Deny),
            _ => Err(format!(
                "invalid policy decision '{s}' (expected allow/ask/deny/classify)"
            )),
        }
    }
}

/// Per-agent tool security policy.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolPolicy {
    pub default: PolicyDecision,
    pub tools: BTreeMap<String, PolicyDecision>,
}

impl ToolPolicy {
    pub fn new(default: PolicyDecision) -> Self {
        Self {
            default,
            tools: BTreeMap::new(),
        }
    }

    /// Default policy matching the current hardcoded behavior.
    pub fn hardcoded_default() -> Self {
        let mut tools = BTreeMap::new();
        tools.insert("shell_session_list".into(), PolicyDecision::Allow);
        tools.insert("shell_session_capture".into(), PolicyDecision::Allow);
        tools.insert("shell_session_create".into(), PolicyDecision::Allow);
        tools.insert("shell_session_kill".into(), PolicyDecision::Ask);
        tools.insert("shell_session_restart".into(), PolicyDecision::Ask);
        tools.insert("shell_session_exec".into(), PolicyDecision::Classify);
        tools.insert("context_pin".into(), PolicyDecision::Allow);
        tools.insert("context_unpin".into(), PolicyDecision::Allow);
        tools.insert("context_status".into(), PolicyDecision::Allow);
        tools.insert("context_evict".into(), PolicyDecision::Allow);
        tools.insert("skill_load".into(), PolicyDecision::Allow);
        Self {
            default: PolicyDecision::Ask,
            tools,
        }
    }

    /// Look up the decision for a tool name.
    pub fn decision_for(&self, tool_name: &str) -> PolicyDecision {
        self.tools.get(tool_name).copied().unwrap_or(self.default)
    }

    /// Validate that this policy is at least as strict as `other`.
    /// Checks the union of both maps' keys plus the default.
    pub fn validate_at_least_as_strict_as(&self, other: &ToolPolicy) -> Result<(), Vec<String>> {
        let mut violations = Vec::new();

        if self.default < other.default {
            violations.push(format!(
                "default {} is less strict than parent's {}",
                self.default, other.default,
            ));
        }

        let all_keys: std::collections::BTreeSet<&String> =
            self.tools.keys().chain(other.tools.keys()).collect();

        for key in &all_keys {
            let mine = self.decision_for(key);
            let theirs = other.decision_for(key);
            if mine < theirs {
                violations.push(format!(
                    "{key}: {} is less strict than parent's {}",
                    mine, theirs,
                ));
            }
        }

        if violations.is_empty() {
            Ok(())
        } else {
            Err(violations)
        }
    }
}

impl Default for ToolPolicy {
    fn default() -> Self {
        Self::hardcoded_default()
    }
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
}

/// Request body for sending a message to an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRequest {
    pub text: String,
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

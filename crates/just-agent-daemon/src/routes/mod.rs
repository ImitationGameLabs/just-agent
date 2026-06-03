mod agent;
pub use agent::restore_sessions;
mod context;
mod approval;
mod message;

use axum::Router;
use just_agent_common::types::AgentId;
use serde::{Deserialize, Serialize};
use state::SharedState;

use crate::state;

#[derive(Debug, Serialize)]
pub struct ListAgentsResponse {
    pub agents: Vec<state::AgentSummary>,
}

#[derive(Debug, Deserialize)]
pub struct MessageRequest {
    pub text: String,
}

#[derive(Debug, Deserialize)]
pub struct ListApprovalsQuery {
    pub offset: Option<u64>,
    /// Page size. Clamped to [1, 20] by the handler; defaults to 5.
    pub limit: Option<u64>,
    pub requested_by: Option<AgentId>,
    pub status: Option<String>,
    pub order: Option<String>,
}

/// Build the full axum router with all agent routes.
pub fn router() -> Router<SharedState> {
    Router::new()
        .route(
            "/agents",
            axum::routing::post(agent::create_agent).get(agent::list_agents),
        )
        .route(
            "/agents/{id}/message",
            axum::routing::post(message::send_message),
        )
        .route(
            "/agents/{id}/events",
            axum::routing::get(message::sse_events),
        )
        .route("/agents/{id}", axum::routing::delete(agent::delete_agent))
        .route(
            "/agents/{id}/interrupt",
            axum::routing::post(agent::interrupt_agent),
        )
        .route(
            "/agents/{id}/status",
            axum::routing::get(context::agent_status),
        )
        .route(
            "/agents/{id}/permissions",
            axum::routing::get(context::agent_permissions),
        )
        .route(
            "/agents/{id}/policy",
            axum::routing::get(context::get_policy).put(context::update_policy),
        )
        .route(
            "/approvals",
            axum::routing::get(approval::list_approvals),
        )
        .route(
            "/approvals/{id}",
            axum::routing::get(approval::get_approval)
                .post(approval::respond_approval),
        )
}

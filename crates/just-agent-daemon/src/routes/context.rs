use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use just_agent_common::context::ContextUsage;
use just_agent_common::retry::RetryRecord;
use just_agent_common::types::AgentId;
use just_agent_common::types::AgentState;
use just_agent_runtime::context::AgenticContext;

use crate::state::SharedState;
use serde::Serialize;

/// Combined status response: context usage + recent retry history.
#[derive(Serialize)]
pub struct AgentStatus {
    pub state: AgentState,
    pub context: ContextUsage,
    pub recent_retries: Vec<RetryRecord>,
}

/// GET /agents/{id}/status — return context usage and retry history.
/// Any authenticated identity may query any agent's status.
pub async fn agent_status(
    State(state): State<SharedState>,
    _auth: crate::auth::AuthIdentity,
    Path(id): Path<AgentId>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let registry = state.registry.read().await;
    let entry = registry
        .get(&id)
        .ok_or((StatusCode::NOT_FOUND, "agent not found".into()))?;
    let store = entry.agent.store.lock().await;
    let context = store.usage_snapshot();
    let recent_retries = store
        .retry_log
        .iter()
        .rev()
        .take(20)
        .cloned()
        .collect::<Vec<_>>();
    Ok(Json(AgentStatus {
        state: entry.agent.get_state(),
        context,
        recent_retries,
    }))
}

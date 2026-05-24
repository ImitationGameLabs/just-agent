use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use just_agent_core::types::SseEvent;

use super::ApprovalRequest;
use crate::state::SharedState;

pub async fn respond_approval(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(req): Json<ApprovalRequest>,
) -> Result<StatusCode, StatusCode> {
    let agents = state.agents.read().await;
    let entry = agents
        .iter()
        .find(|e| e.id == id)
        .ok_or(StatusCode::NOT_FOUND)?;

    let mut deferred = entry.agent.deferred.lock().await;
    match req.decision.as_str() {
        "approve" => {
            deferred.approve(&req.request_id).map_err(|_| StatusCode::NOT_FOUND)?;
            entry.agent.events_tx.send(SseEvent::DeferredApproved {
                request_id: req.request_id.clone(),
            }).ok();
            Ok(StatusCode::OK)
        }
        "deny" => {
            let reason = req.reason.as_deref().unwrap_or("denied").to_owned();
            deferred.deny(&req.request_id, &reason).map_err(|_| StatusCode::NOT_FOUND)?;
            entry.agent.events_tx.send(SseEvent::DeferredDenied {
                request_id: req.request_id.clone(),
                reason,
            }).ok();
            Ok(StatusCode::OK)
        }
        _ => Err(StatusCode::BAD_REQUEST),
    }
}

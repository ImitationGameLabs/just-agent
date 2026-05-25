use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use just_agent_core::persistence;
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

    {
        let mut deferred = entry.agent.deferred.lock().await;
        let json = match req.decision.as_str() {
            "approve" => {
                deferred
                    .approve(&req.request_id)
                    .map_err(|_| StatusCode::NOT_FOUND)?;
                entry
                    .agent
                    .events_tx
                    .send(SseEvent::DeferredApproved { request_id: req.request_id.clone() })
                    .ok();
                serde_json::to_string(&*deferred).ok()
            }
            "deny" => {
                let reason = req.reason.as_deref().unwrap_or("denied").to_owned();
                deferred
                    .deny(&req.request_id, &reason)
                    .map_err(|_| StatusCode::NOT_FOUND)?;
                entry
                    .agent
                    .events_tx
                    .send(SseEvent::DeferredDenied { request_id: req.request_id.clone(), reason })
                    .ok();
                serde_json::to_string(&*deferred).ok()
            }
            _ => return Err(StatusCode::BAD_REQUEST),
        };

        // Persist while still holding the lock so the agent loop's
        // concurrent persist() cannot interleave a stale write.
        if let (Some(json), Some(dir)) = (json, entry.agent.session_dir.as_ref())
            && let Err(e) = persistence::persist_deferred(&json, dir)
        {
            tracing::error!("deferred persist after approval decision failed: {e:#}");
        }
    }

    Ok(StatusCode::OK)
}

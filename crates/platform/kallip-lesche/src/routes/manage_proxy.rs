//! Manage-plane reverse proxy: `/v1/tagma/{agent}/manage/{*path}`.
//!
//! Bridges the plaintext manage surface (api-redesign §9.3): an
//! authenticated operator session is checked against the tunnel's owner
//! (arch C1 -- the only application-layer authorization, replacing the
//! authorization boundary the E2EE envelope used to provide), then the
//! request is fanned down the tagma's tunnel as a
//! [`TunnelInbound::ManageRest`] frame and the plaintext reply POST is
//! awaited. Tunnel offline degrades to an immediate 502; a dropped reply
//! degrades to a 504 after the wait window.

use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::any;
use kallip_archeion_common::ids::{ParticipantId, TagmaId};
use kallip_archeion_common::principal::Principal;
use kallip_lesche_common::tunnel::{ManageRestReply, TunnelInbound};
use std::collections::HashMap;
use std::sync::OnceLock;
use tokio::sync::{Mutex, oneshot};

use crate::auth::AuthPrincipal;
use crate::state::SharedConvState;

/// How long the proxy waits for the tagma's plaintext reply before giving up
/// (504). Shorter than the old envelope-path 15s blind wait by design.
const REPLY_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// In-flight (tagma, req_id) -> reply channel. A request is registered before
/// the frame is fanned and resolved by the `/v1/tunnel/manage-reply` POST.
fn pending() -> &'static Mutex<HashMap<(TagmaId, u64), oneshot::Sender<ManageRestReply>>> {
    static PENDING: OnceLock<Mutex<HashMap<(TagmaId, u64), oneshot::Sender<ManageRestReply>>>> =
        OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Resolve a pending proxy request from the tagma's manage-reply POST.
/// Returns false when no waiter matches (unknown req_id or late duplicate).
pub async fn resolve(tagma_id: &TagmaId, req_id: u64, reply: ManageRestReply) -> bool {
    let mut pending = pending().lock().await;
    pending
        .remove(&(tagma_id.clone(), req_id))
        .map(|tx| tx.send(reply).is_ok())
        .unwrap_or(false)
}

pub fn router() -> Router<SharedConvState> {
    Router::new().route("/v1/tagma/{agent}/manage/{*path}", any(proxy_manage))
}

async fn proxy_manage(
    State(state): State<SharedConvState>,
    AuthPrincipal(principal): AuthPrincipal,
    Path(agent): Path<String>,
    method: axum::http::Method,
    uri: axum::http::Uri,
    body: Option<axum::Json<serde_json::Value>>,
) -> Result<axum::response::Response, ApiErr> {
    let user = match principal {
        Principal::User(user_id) => user_id,
        // Tagma-bearer callers have no business on the operator proxy.
        _ => return Ok((StatusCode::FORBIDDEN, "operator session required").into_response()),
    };
    let tagma_id = TagmaId::from(agent.clone());

    // arch C1: tenant authorization. The E2EE envelope used to carry this
    // boundary implicitly; on the plaintext frame it must be explicit.
    let tx = {
        let registry = state.registry.read().expect("registry lock");
        match registry.presence.get(&ParticipantId::for_tagma(&tagma_id)) {
            Some(entry) if entry.owner == user => entry.tx.clone(),
            Some(_) => {
                return Ok((StatusCode::FORBIDDEN, "not your tagma").into_response());
            }
            None => {
                return Ok((StatusCode::NOT_FOUND, "tagma not online").into_response());
            }
        }
    };

    // Contract (root-approved): no query string on the frame surface.
    if uri.query().is_some() {
        return Ok((StatusCode::NOT_FOUND, "query not allowed on the frame").into_response());
    }
    let path = uri.path();
    let req_id = next_req_id();

    let (reply_tx, reply_rx) = oneshot::channel();
    pending()
        .lock()
        .await
        .insert((tagma_id.clone(), req_id), reply_tx);

    let frame = TunnelInbound::ManageRest {
        req_id,
        method: method.as_str().to_ascii_uppercase(),
        path: path.to_owned(),
        body: body
            .map(|axum::Json(v)| v)
            .unwrap_or(serde_json::Value::Null),
    };
    if tx.send(frame).is_err() {
        pending().lock().await.remove(&(tagma_id, req_id));
        return Ok((StatusCode::BAD_GATEWAY, "tagma tunnel offline").into_response());
    }

    match tokio::time::timeout(REPLY_WAIT, reply_rx).await {
        Ok(Ok(reply)) => Ok((
            StatusCode::from_u16(reply.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            axum::Json(reply.body),
        )
            .into_response()),
        _ => {
            // Slow-leak guard: a timed-out or dropped waiter must not leave
            // its entry in the pending map (req_id never repeats).
            pending().lock().await.remove(&(tagma_id, req_id));
            Ok((StatusCode::GATEWAY_TIMEOUT, "manage reply not received").into_response())
        }
    }
}

static REQ_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn next_req_id() -> u64 {
    REQ_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

// Placeholder to keep the error alias referenced; the real response error type
// is fully inline above (every path returns a concrete Response).
type ApiErr = (StatusCode, &'static str);

#[cfg(test)]
mod tests {
    use super::*;

    /// Serialization round-trip: the reply must survive the wire both ways
    /// (lesche -> tagma frame -> tagma -> plaintext reply POST).
    #[test]
    fn manage_rest_frame_and_reply_round_trip() {
        let frame = TunnelInbound::ManageRest {
            req_id: 7,
            method: "GET".to_owned(),
            path: "/agents".to_owned(),
            body: serde_json::Value::Null,
        };
        let encoded = serde_json::to_string(&frame).expect("frame encodes");
        assert!(encoded.contains("manage_rest"));

        let reply = ManageRestReply {
            req_id: 7,
            status: 200,
            body: serde_json::json!({"ok": true}),
        };
        let encoded = serde_json::to_string(&reply).expect("reply encodes");
        let decoded: ManageRestReply = serde_json::from_str(&encoded).expect("reply decodes");
        assert_eq!(decoded.req_id, 7);
        assert_eq!(decoded.status, 200);
    }

    /// Pending bookkeeping: register -> resolve consumes exactly once.
    #[tokio::test]
    async fn pending_resolution_is_single_shot() {
        let tagma = TagmaId::from("t-1".to_string());
        let (tx, rx) = oneshot::channel();
        pending().lock().await.insert((tagma.clone(), 42), tx);
        assert!(
            resolve(
                &tagma,
                42,
                ManageRestReply {
                    req_id: 42,
                    status: 200,
                    body: serde_json::json!({}),
                }
            )
            .await
        );
        // Second resolve: already consumed.
        assert!(
            !resolve(
                &tagma,
                42,
                ManageRestReply {
                    req_id: 42,
                    status: 200,
                    body: serde_json::json!({}),
                }
            )
            .await
        );
        let _ = rx.await;
    }
}

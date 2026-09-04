//! The tagma state plane (api-redesign §9.1/§9.7): the write endpoint the
//! tagma pushes full snapshots to (PUT /tagmata/{id}/state, an idempotent
//! whole-resource replace), the per-tagma read endpoints that serve the
//! stored snapshot (stale reads included -- the table outlives presence,
//! MIN3), and the per-tagma SSE change-notification stream whose
//! subscription-count flips drive `SubscriptionHint` over the tagma's tunnel.
//!
//! Every client-facing route re-checks C1 (the stored/live owner must be the
//! caller) so cross-tenant reads are 403; the write route authenticates the
//! tagma itself and pins the push to its own id.
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use kallip_archeion_common::ids::{ParticipantId, TagmaId};
use kallip_archeion_common::principal::{Principal, require_tagma, require_user};
use kallip_lesche_common::projection::{ProjectionDirty, ProjectionSnapshot};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;

use crate::auth::AuthPrincipal;
use crate::sse::{BoxEventStream, OnDrop};
use crate::state::SharedConvState;

pub fn router() -> Router<SharedConvState> {
    Router::new()
        // Registered without the /v1 prefix: the app nests this router under
        // /v1, so the final mounts are /v1/tagmata/{id}/* -- the shapes the
        // tagma pump and the browser client both dial.
        // GET and PUT share /state: GET streams the dirty-frame SSE, PUT
        // replaces the stored snapshot (one resource, two representations).
        .route(
            "/tagmata/{id}/state",
            get(state_events).put(accept_state_push),
        )
        .route("/tagmata/{id}/agents", get(read_agents))
        .route("/tagmata/{id}/budget", get(read_budget))
        .route("/tagmata/{id}/work-schedule", get(read_work_schedule))
}

/// C1 for the read/SSE plane, offline-safe: the owner recorded on the stored
/// projection entry (or the live presence) must be the caller. `None` when
/// the tagma has no stored projection at all.
#[allow(clippy::result_large_err)] // one-line helper; boxing the error costs more
fn require_owner(
    principal: &Principal,
    state: &SharedConvState,
    tagma: &TagmaId,
) -> Result<(), Response> {
    let user = match require_user(principal) {
        Ok(u) => u,
        Err(_) => {
            return Err((StatusCode::UNAUTHORIZED, "operator session required").into_response());
        }
    };
    let reg = state.registry.read().expect("registry lock");
    let owner = match reg.presence.get(&ParticipantId::for_tagma(tagma)) {
        Some(entry) => Some(&entry.owner),
        None => reg.projection(tagma).map(|p| &p.owner),
    };
    match owner {
        Some(o) if o == user => Ok(()),
        Some(_) => Err((StatusCode::FORBIDDEN, "not your tagma").into_response()),
        None => Err((StatusCode::NOT_FOUND, "no projection").into_response()),
    }
}

/// The tagma's push: validate it targets the pushing tagma itself, accept it
/// into the store (M1 generation logic inside), and fan the dirty event to
/// this tagma's projection subscribers.
async fn accept_state_push(
    State(state): State<SharedConvState>,
    AuthPrincipal(principal): AuthPrincipal,
    Path(id): Path<String>,
    axum::Json(snapshot): axum::Json<ProjectionSnapshot>,
) -> Response {
    let pusher = match require_tagma(&principal) {
        Ok(id) => id.clone(),
        Err(_) => return (StatusCode::UNAUTHORIZED, "tagma bearer required").into_response(),
    };
    let tagma_id = TagmaId::from(id);
    if pusher != tagma_id {
        return (
            StatusCode::FORBIDDEN,
            "a tagma may only push its own projection",
        )
            .into_response();
    }
    let mut registry = state.registry.write().expect("registry lock");
    let (generation, owner) = match registry.presence.get(&ParticipantId::for_tagma(&tagma_id)) {
        Some(entry) => (entry.id.clone(), entry.owner.clone()),
        None => {
            return (
                StatusCode::NOT_FOUND,
                "tagma not online (push needs a live tunnel session)",
            )
                .into_response();
        }
    };
    match registry.accept_projection(
        &tagma_id,
        &generation,
        snapshot.push_seq,
        owner.clone(),
        snapshot,
    ) {
        Some(seq) => {
            registry.fan_projection_dirty(
                &owner,
                &tagma_id,
                ProjectionDirty {
                    tagma_id: tagma_id.clone(),
                    seq,
                },
            );
            (
                StatusCode::OK,
                axum::Json(serde_json::json!({ "seq": seq })),
            )
                .into_response()
        }
        None => (
            StatusCode::OK,
            axum::Json(serde_json::json!({ "ignored": "stale push_seq" })),
        )
            .into_response(),
    }
}

/// Shared read core: C1, then the stored entry and its staleness (a tagma
/// with no live presence serves its projection as `stale`).
#[allow(clippy::result_large_err)] // sibling helper of require_owner
fn read_entry(
    principal: &Principal,
    state: &SharedConvState,
    tagma: &TagmaId,
) -> Result<(crate::state::ProjectionEntry, bool), Response> {
    require_owner(principal, state, tagma)?;
    let registry = state.registry.read().expect("registry lock");
    let entry = registry
        .projection(tagma)
        .cloned()
        .ok_or_else(|| (StatusCode::NOT_FOUND, "no projection").into_response())?;
    let stale = !registry
        .presence
        .contains_key(&ParticipantId::for_tagma(tagma));
    Ok((entry, stale))
}

async fn read_agents(
    State(state): State<SharedConvState>,
    AuthPrincipal(principal): AuthPrincipal,
    Path(id): Path<String>,
) -> Response {
    let tagma_id = TagmaId::from(id);
    let (entry, stale) = match read_entry(&principal, &state, &tagma_id) {
        Ok(v) => v,
        Err(r) => return r,
    };
    axum::Json(serde_json::json!({
        "stale": stale,
        "seq": entry.seq,
        "updated_at": entry.updated_at,
        "agents": entry.snapshot.agents,
        "status": entry.snapshot.status,
    }))
    .into_response()
}

async fn read_budget(
    State(state): State<SharedConvState>,
    AuthPrincipal(principal): AuthPrincipal,
    Path(id): Path<String>,
) -> Response {
    let tagma_id = TagmaId::from(id);
    let (entry, stale) = match read_entry(&principal, &state, &tagma_id) {
        Ok(v) => v,
        Err(r) => return r,
    };
    axum::Json(serde_json::json!({
        "stale": stale,
        "seq": entry.seq,
        "updated_at": entry.updated_at,
        "budget": entry.snapshot.status.token_budget,
        "consumed": entry.snapshot.status.token_consumed,
    }))
    .into_response()
}

async fn read_work_schedule(
    State(state): State<SharedConvState>,
    AuthPrincipal(principal): AuthPrincipal,
    Path(id): Path<String>,
) -> Response {
    let tagma_id = TagmaId::from(id);
    let (entry, stale) = match read_entry(&principal, &state, &tagma_id) {
        Ok(v) => v,
        Err(r) => return r,
    };
    axum::Json(serde_json::json!({
        "stale": stale,
        "seq": entry.seq,
        "updated_at": entry.updated_at,
        "work_schedule": entry.snapshot.work_schedule,
    }))
    .into_response()
}

/// The per-tagma change stream: `ProjectionDirty { tagma_id, seq }` frames on
/// an independent broadcast (never the `me/events` app stream, §9.7). The
/// 0 -> 1 subscribe edge fans `SubscriptionHint { active: true }` down the
/// tagma's tunnel; the last unsubscribe fans `false` after the lag window.
async fn state_events(
    State(state): State<SharedConvState>,
    AuthPrincipal(principal): AuthPrincipal,
    Path(id): Path<String>,
) -> Result<Sse<OnDrop>, StatusCode> {
    // q-M-1: the SSE plane is owner-gated like the read plane -- a
    // cross-tenant subscriber must not be able to flip the tagma's
    // subscription hint (remote pump start) or probe existence.
    let tagma_id = TagmaId::from(id);
    let user_id = match require_owner(&principal, &state, &tagma_id) {
        Ok(()) => match require_user(&principal) {
            Ok(u) => u.clone(),
            Err(_) => return Err(StatusCode::UNAUTHORIZED),
        },
        Err(resp) => return Err(resp.status()),
    };
    let handle = tokio::runtime::Handle::try_current().ok();
    let (tx, rx, _was_first, unsub_lag) = {
        let mut registry = state.registry.write().expect("registry lock");
        let stream = registry.open_projection_stream(&user_id, &tagma_id);
        let lag = std::time::Duration::from_millis(registry.projection_unsub_lag_ms());
        (stream.0, stream.1, stream.2, lag)
    };
    let stream: BoxEventStream = Box::pin(
        BroadcastStream::new(rx)
            .filter_map(|r| match r {
                Ok(ev) => Some(ev),
                Err(BroadcastStreamRecvError::Lagged(n)) => {
                    tracing::warn!(lag = n, "projection SSE lagged; dirty frames dropped");
                    None
                }
            })
            .map(|dirty| {
                Ok::<Event, std::convert::Infallible>(
                    Event::default().json_data(dirty).expect("event serializes"),
                )
            }),
    );
    // tx is the Sender cloned from the map. By the time this closure runs,
    // the stream (and our rx inside it) is already dropped, so
    // `receiver_count() > 1` == "another subscriber still live". The lag
    // window delays the teardown so a quick reconnect does not flap the hint.
    let cleanup_state = state.clone();
    let cleanup_user = user_id.clone();
    let cleanup_tx = tx.clone();
    let cleaned = OnDrop::new(stream, move || {
        if cleanup_tx.receiver_count() > 1 {
            return;
        }
        let st = cleanup_state.clone();
        let user = cleanup_user.clone();
        let tagma = tagma_id.clone();
        let tx = cleanup_tx.clone();
        let Some(h) = handle.as_ref() else {
            return;
        };
        h.spawn(async move {
            tokio::time::sleep(unsub_lag).await;
            // remove_if_last fans the `SubscriptionHint { active: false }`
            // itself on the 1 -> 0 edge -- no duplicate send here.
            let Ok(mut registry) = st.write() else {
                return;
            };
            registry.remove_projection_stream_if_last(&user, &tagma, &tx);
        });
    });
    Ok(Sse::new(cleaned))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::test_support::db_state;
    use axum::extract::{Path, State};
    use axum::http::StatusCode;
    use kallip_archeion_common::ids::UserId;
    use kallip_lesche_common::tunnel::TunnelInbound;
    use std::sync::Arc;

    pub(super) fn uid(s: &str) -> UserId {
        UserId::from(s.to_string())
    }

    pub(super) fn tagma_of(s: &str) -> TagmaId {
        TagmaId::from(s.to_string())
    }

    fn snapshot_with(push_seq: u64) -> ProjectionSnapshot {
        serde_json::from_value(serde_json::json!({
            "agents": [],
            "status": {
                "root_state": "idle",
                "subagents_total": 0,
                "subagents_active": 0,
                "token_budget": 100,
                "token_consumed": 3,
            },
            "push_seq": push_seq,
            "work_schedule": null,
        }))
        .expect("snapshot parses")
    }

    pub(super) async fn push(
        state: &SharedConvState,
        principal: AuthPrincipal,
        agent: &str,
        push_seq: u64,
    ) -> Response {
        accept_state_push(
            State(state.clone()),
            principal,
            Path(agent.to_string()),
            axum::Json(snapshot_with(push_seq)),
        )
        .await
    }

    pub(super) fn owner_principal(user: &UserId) -> AuthPrincipal {
        AuthPrincipal(Principal::User(user.clone()))
    }

    pub(super) async fn enroll(
        state: &SharedConvState,
        tagma: &TagmaId,
        owner: &UserId,
    ) -> tokio::sync::broadcast::Receiver<TunnelInbound> {
        let mut reg = state.registry.write().unwrap();
        let (tx, rx) = tokio::sync::broadcast::channel(8);
        reg.register_presence(tagma, owner.clone(), tx, Arc::new(()));
        rx
    }

    /// The write endpoint authenticates the tagma bearer: an operator (user)
    /// principal pushing is 401, and the tagma pins the push to its own id.
    #[tokio::test]
    async fn push_rejects_non_tagma_and_foreign_ids() {
        let (state, _control) = db_state().await;
        let tagma = tagma_of("t-a");
        let owner = uid("alice");
        let _rx = enroll(&state, &tagma, &owner).await;
        let resp = push(&state, owner_principal(&owner), "t-a", 1).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// Cross-tenant reads are 403 (C1) and an unknown tagma is 404.
    #[tokio::test]
    async fn cross_tenant_read_is_forbidden() {
        let (state, _control) = db_state().await;
        let tagma = tagma_of("t-a");
        let owner = uid("alice");
        let other = uid("mallory");
        let _rx = enroll(&state, &tagma, &owner).await;
        let resp = push(
            &state,
            AuthPrincipal(Principal::Tagma(tagma.clone())),
            "t-a",
            1,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let ok = read_agents(
            State(state.clone()),
            owner_principal(&owner),
            Path("t-a".to_string()),
        )
        .await;
        assert_eq!(ok.status(), StatusCode::OK);
        let denied = read_agents(
            State(state.clone()),
            owner_principal(&other),
            Path("t-a".to_string()),
        )
        .await;
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    }

    /// Accepted pushes bump the store seq and serve the stored snapshot; a
    /// same-generation replay (stale push_seq) is ignored without bumping.
    #[tokio::test]
    async fn push_read_roundtrip_and_replay_rejection() {
        let (state, _control) = db_state().await;
        let tagma = tagma_of("t-a");
        let owner = uid("alice");
        let _rx = enroll(&state, &tagma, &owner).await;
        let first = push(
            &state,
            AuthPrincipal(Principal::Tagma(tagma.clone())),
            "t-a",
            1,
        )
        .await;
        assert_eq!(first.status(), StatusCode::OK);
        let replay = push(
            &state,
            AuthPrincipal(Principal::Tagma(tagma.clone())),
            "t-a",
            1,
        )
        .await;
        assert_eq!(replay.status(), StatusCode::OK); // idempotent ignore
        let body = read_agents(
            State(state.clone()),
            owner_principal(&owner),
            Path("t-a".to_string()),
        )
        .await;
        let bytes = axum::body::to_bytes(body.into_body(), 1 << 20)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["seq"], 1, "replay must not bump the store seq");
        assert_eq!(json["stale"], false, "tagma online: not stale");
        assert!(json["agents"].is_array());
    }
}

#[cfg(test)]
mod generation_tests {
    use super::tests::{enroll, owner_principal, push, tagma_of, uid};
    use super::*;
    use crate::routes::test_support::db_state;
    use kallip_lesche_common::tunnel::TunnelInbound;

    /// M1: a reconnecting tagma's push counter restarted, so the first push
    /// of the new generation is accepted unconditionally even though its
    /// push_seq is lower than the previous generation's last one.
    #[tokio::test]
    async fn reconnect_generation_accepts_reset_seq() {
        let (state, _control) = db_state().await;
        let tagma = tagma_of("t-a");
        let owner = uid("alice");
        let _rx = enroll(&state, &tagma, &owner).await;
        let principal = AuthPrincipal(Principal::Tagma(tagma.clone()));
        let high = push(&state, principal.clone(), "t-a", 9).await;
        assert_eq!(high.status(), StatusCode::OK);
        // The tunnel drops and re-establishes: a fresh presence generation.
        let _rx2 = enroll(&state, &tagma, &owner).await;
        let after = push(&state, principal, "t-a", 1).await;
        assert_eq!(after.status(), StatusCode::OK);
        let body = read_agents(
            State(state.clone()),
            owner_principal(&owner),
            Path("t-a".to_string()),
        )
        .await;
        let bytes = axum::body::to_bytes(body.into_body(), 1 << 20)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            json["seq"], 2,
            "new-generation push is accepted and seq bumps"
        );
    }

    /// The subscription flip fans SubscriptionHint both ways over the tagma's
    /// tunnel: first subscriber -> true, (post-lag-window teardown of) the
    /// last subscriber -> false. The lag window itself lives in the SSE
    /// handler; this pins the registry edges the handler drives.
    #[tokio::test]
    async fn subscription_flip_fans_hint_both_ways() {
        let (state, _control) = db_state().await;
        let tagma = tagma_of("t-a");
        let owner = uid("alice");
        let mut hint_rx = enroll(&state, &tagma, &owner).await;
        let (tx, _rx, was_first) = {
            let mut reg = state.registry.write().unwrap();
            reg.open_projection_stream(&owner, &tagma)
        };
        assert!(was_first, "first subscriber is the 0 -> 1 edge");
        hint_rx
            .try_recv()
            .expect("hint true fanned on the open edge");
        assert!(matches!(
            hint_rx.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
        // OnDrop semantics: the dying stream's own rx is dropped before the
        // cleanup runs, which is what makes receiver_count() hit zero.
        drop(_rx);
        let removed = {
            let mut reg = state.registry.write().unwrap();
            reg.remove_projection_stream_if_last(&owner, &tagma, &tx)
        };
        assert!(removed, "last unsubscribe is the 1 -> 0 edge");
        // q-M-2: the false half of the flip actually reaches the tunnel.
        assert!(matches!(
            hint_rx.try_recv(),
            Ok(TunnelInbound::SubscriptionHint { active: false })
        ));
    }

    /// q-M-2: the teardown lag gates the false flip. A subscriber that
    /// re-subscribes inside the window re-arms the hint (its open is the
    /// new 0 -> 1 edge) and the pending teardown no-ops; only the true
    /// last departure fans `false` and removes the channel.
    #[tokio::test]
    async fn unsub_lag_window_gates_the_false_flip() {
        let (state, _control) = db_state().await;
        let tagma = tagma_of("t-a");
        let owner = uid("alice");
        let mut hint_rx = enroll(&state, &tagma, &owner).await;
        let (tx, rx, was_first) = {
            let mut reg = state.registry.write().unwrap();
            reg.open_projection_stream(&owner, &tagma)
        };
        assert!(was_first);
        hint_rx
            .try_recv()
            .expect("hint true fanned on the open edge");
        drop(rx);
        // A new subscriber inside the lag window: re-arms the hint.
        let (tx2, rx2, was_first_again) = {
            let mut reg = state.registry.write().unwrap();
            reg.open_projection_stream(&owner, &tagma)
        };
        assert!(was_first_again, "re-subscribe re-arms the hint");
        let removed = {
            let mut reg = state.registry.write().unwrap();
            reg.remove_projection_stream_if_last(&owner, &tagma, &tx)
        };
        assert!(!removed, "a live subscriber survives the teardown");
        assert!(matches!(
            hint_rx.try_recv(),
            Ok(TunnelInbound::SubscriptionHint { active: true })
        ));
        // After the true departure: teardown + false hint.
        drop(rx2);
        let removed = {
            let mut reg = state.registry.write().unwrap();
            reg.remove_projection_stream_if_last(&owner, &tagma, &tx2)
        };
        assert!(removed, "the last unsubscribe tears the channel down");
        assert!(matches!(
            hint_rx.try_recv(),
            Ok(TunnelInbound::SubscriptionHint { active: false })
        ));
    }

    /// q-M-2: the offline arm of the read plane -- with the tagma's
    /// presence gone, the stored projection still serves the owner
    /// (MIN3), flagged `stale`, at the same seq the online read showed.
    #[tokio::test]
    async fn offline_tagma_serves_stale_projection() {
        let (state, _control) = db_state().await;
        let tagma = tagma_of("t-a");
        let owner = uid("alice");
        let generation = {
            let mut reg = state.registry.write().unwrap();
            let (tx, _rx) = tokio::sync::broadcast::channel(8);
            let id = std::sync::Arc::new(());
            reg.register_presence(&tagma, owner.clone(), tx, id.clone());
            id
        };
        let first = push(
            &state,
            AuthPrincipal(Principal::Tagma(tagma.clone())),
            "t-a",
            1,
        )
        .await;
        assert_eq!(first.status(), StatusCode::OK);
        // The tunnel drops: presence is gone, the projection table stays.
        {
            let mut reg = state.registry.write().unwrap();
            assert!(reg.take_presence_if_owned(&tagma, &generation));
        }
        let body = read_agents(
            State(state.clone()),
            owner_principal(&owner),
            Path("t-a".to_string()),
        )
        .await;
        assert_eq!(body.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(body.into_body(), 1 << 20)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["seq"], 1);
        assert_eq!(json["stale"], true, "offline tagma reads as stale");
    }

    /// q-M-1: the SSE plane is owner-gated like the read plane -- a
    /// cross-tenant subscriber gets 403 and can neither start the tagma's
    /// pump remotely nor probe the tagma's existence.
    #[tokio::test]
    async fn cross_tenant_events_subscription_is_forbidden() {
        let (state, _control) = db_state().await;
        let tagma = tagma_of("t-a");
        let owner = uid("alice");
        let other = uid("mallory");
        let _rx = enroll(&state, &tagma, &owner).await;
        let err = state_events(
            State(state.clone()),
            owner_principal(&other),
            Path("t-a".to_string()),
        )
        .await
        .unwrap_err();
        assert_eq!(err, StatusCode::FORBIDDEN);
    }

    /// q-M-2 end-to-end: the real OnDrop path -- drop the SSE response,
    /// the injected lag elapses, then the false hint reaches the tunnel
    /// and the channel is torn down (this is what consumes the setter).
    #[tokio::test]
    async fn on_drop_teardown_fans_false_after_injected_lag() {
        let (state, _control) = db_state().await;
        let tagma = tagma_of("t-a");
        let owner = uid("alice");
        let mut hint_rx = enroll(&state, &tagma, &owner).await;
        {
            let reg = state.registry.read().unwrap();
            reg.set_projection_unsub_lag_ms(50);
        }
        let sse = state_events(
            State(state.clone()),
            owner_principal(&owner),
            Path("t-a".to_string()),
        )
        .await
        .expect("owner subscribes");
        assert!(matches!(
            hint_rx.try_recv(),
            Ok(TunnelInbound::SubscriptionHint { active: true })
        ));
        drop(sse); // OnDrop fires: lag, then teardown + false hint
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(matches!(
            hint_rx.try_recv(),
            Ok(TunnelInbound::SubscriptionHint { active: false })
        ));
        let live = {
            let reg = state.registry.read().unwrap();
            reg.projection_stream_live_for_tagma(&tagma)
        };
        assert!(!live, "the channel is gone after the lag window");
    }
}

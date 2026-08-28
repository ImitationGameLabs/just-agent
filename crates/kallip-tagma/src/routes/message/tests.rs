use super::*;
use crate::auth::{AuthIdentity, Identity};
use crate::state::{AgentId, RegistryEntry};
use crate::test_helpers::{
    add_faulted_root, add_root, add_sub, install_inbox_store, make_entry_with_rx, make_state,
};
use axum::Json;
use axum::extract::{Path, State};
use kallip_common::protocol::MessageRequest;

// -- send_message: sender identity is attached to the delivered payload --

/// Deliver a message as the operator. The full envelope (with header) is
/// stored in the inbox.
#[tokio::test]
async fn operator_message_stores_envelope_in_inbox() {
    let state = make_state();
    install_inbox_store(&state).await;
    let receiver = AgentId::random();
    let (mut entry, _rx) = make_entry_with_rx(None, "recv".into());
    entry.identity.config.role = "root".into();
    state
        .registry
        .write()
        .await
        .register(receiver.clone(), RegistryEntry::Live(entry));

    let resp = send_message(
        State(state.clone()),
        AuthIdentity::test_new(Identity::Operator),
        Path(receiver.clone()),
        Json(MessageRequest {
            text: "do the thing".into(),
        }),
    )
    .await
    .expect("operator send accepted");
    assert_eq!(resp.0, StatusCode::ACCEPTED);

    let msg = state
        .inboxes
        .get()
        .unwrap()
        .pull_undelivered(&receiver)
        .await
        .unwrap();
    assert!(msg.contains("[From: operator]"));
    assert!(msg.contains("do the thing"));
}

/// Deliver a message from a child agent to its parent. The inbox stores
/// the full envelope with sender id + role + relation.
#[tokio::test]
async fn agent_message_stores_sender_and_relation() {
    let state = make_state();
    install_inbox_store(&state).await;
    let parent = AgentId::random();
    let child = AgentId::random();

    let (mut parent_entry, _parent_rx) = make_entry_with_rx(None, "parent".into());
    parent_entry.identity.config.role = "lead".into();
    state
        .registry
        .write()
        .await
        .register(parent.clone(), RegistryEntry::Live(parent_entry));

    let (mut child_entry, _child_rx) = make_entry_with_rx(Some(parent.clone()), "child".into());
    child_entry.identity.config.role = "researcher".into();
    state
        .registry
        .write()
        .await
        .register(child.clone(), RegistryEntry::Live(child_entry));

    let resp = send_message(
        State(state.clone()),
        AuthIdentity::test_new(Identity::Agent { id: child.clone() }),
        Path(parent.clone()),
        Json(MessageRequest {
            text: "results attached".into(),
        }),
    )
    .await
    .expect("agent send accepted");
    assert_eq!(resp.0, StatusCode::ACCEPTED);

    let msg = state
        .inboxes
        .get()
        .unwrap()
        .pull_undelivered(&parent)
        .await
        .unwrap();
    assert!(msg.contains(&child.to_string()));
    assert!(msg.contains("researcher"));
    assert!(msg.contains("results attached"));
}

/// Self-message: an agent messaging itself is stored in the inbox.
#[tokio::test]
async fn self_message_stored_in_inbox() {
    let state = make_state();
    install_inbox_store(&state).await;
    let me = AgentId::random();
    let (mut entry, _rx) = make_entry_with_rx(None, "me".into());
    entry.identity.config.role = "solo".into();
    state
        .registry
        .write()
        .await
        .register(me.clone(), RegistryEntry::Live(entry));

    let _ = send_message(
        State(state.clone()),
        AuthIdentity::test_new(Identity::Agent { id: me.clone() }),
        Path(me.clone()),
        Json(MessageRequest {
            text: "note to self".into(),
        }),
    )
    .await
    .expect("self send accepted");

    let msg = state
        .inboxes
        .get()
        .unwrap()
        .pull_undelivered(&me)
        .await
        .unwrap();
    assert!(msg.contains("note to self"));
}

/// Messaging a faulted agent returns 409 with the reason.
#[tokio::test]
async fn send_message_to_faulted_returns_conflict() {
    let state = make_state();
    install_inbox_store(&state).await;
    let faulted = AgentId::random();
    {
        let mut reg = state.registry.write().await;
        add_faulted_root(&mut reg, &faulted, "restore failed: missing workspace");
    }
    let err = send_message(
        State(state),
        AuthIdentity::test_new(Identity::Operator),
        Path(faulted),
        Json(MessageRequest { text: "hi".into() }),
    )
    .await
    .expect_err("faulted agent rejects messages");
    assert_eq!(err.status, 409);
    assert!(
        err.message.contains("faulted"),
        "message should mention faulted: {}",
        err.message
    );
    assert!(err.message.contains("missing workspace"), "{}", err.message);
}

/// Messaging a parked agent auto-wakes it: the message lands in the inbox
/// and the kick `[system]` turn (same text the deleted wake endpoint sent)
/// is enqueued on the prompt channel — the agent decides, and the runtime's
/// post-round drain then pulls the message. The old behavior (409 with a
/// wake-endpoint hint) is gone.
#[tokio::test]
async fn send_message_to_parked_agent_auto_wakes_with_kick_turn() {
    let state = make_state();
    install_inbox_store(&state).await;
    let root = AgentId::random();
    let parked = AgentId::random();
    {
        let mut reg = state.registry.write().await;
        add_root(&mut reg, &root);
        add_sub(&mut reg, &parked, &root);
        let live = reg.get(&parked).unwrap().as_live().unwrap();
        live.agent.state.store(
            crate::state::AgentState::PARKED,
            std::sync::atomic::Ordering::Relaxed,
        );
        *live.agent.parked.lock().unwrap() = Some(crate::state::ParkedSnapshot {
            reason: kallip_common::protocol::ParkedReason::FatalError {
                message: "boom".to_string(),
            },
            at: std::time::Instant::now(),
        });
    }
    let (prompt_tx, mut prompt_rx) = tokio::sync::mpsc::channel::<String>(4);
    {
        let mut reg = state.registry.write().await;
        let live = reg.get_mut(&parked).unwrap().as_live_mut().unwrap();
        live.agent.prompt_tx = prompt_tx;
    }
    let resp = send_message(
        State(state.clone()),
        AuthIdentity::test_new(Identity::Operator),
        Path(parked.clone()),
        Json(MessageRequest { text: "hi".into() }),
    )
    .await
    .expect("parked agent accepts the message and auto-wakes");
    let (status, Json(body)) = resp;
    assert_eq!(status, axum::http::StatusCode::ACCEPTED);
    assert!(
        body.warning.as_deref().unwrap_or("").contains("kick turn"),
        "the response must note the auto-wake: {:?}",
        body.warning
    );
    let turn = tokio::time::timeout(std::time::Duration::from_millis(500), prompt_rx.recv())
        .await
        .expect("kick turn must be enqueued")
        .expect("prompt channel open");
    assert!(
        turn.starts_with("[system] you were parked") && turn.contains("ago: fatal error: boom"),
        "kick turn must carry the duration and reason: {turn}"
    );
    // The message itself is durable in the inbox — the runtime's
    // post-round drain pulls it once the kick round un-parks the agent.
    assert_eq!(state.inboxes.get().unwrap().len_for(&parked).await, 1);
    assert_eq!(state.inboxes.get().unwrap().len_for(&root).await, 0);
}

/// A parked state without a parked payload is an invariant break: the send
/// is still accepted (the message is safe in the inbox — never failed on an
/// invariant), no kick turn is fabricated, and the warning names it.
#[tokio::test]
async fn send_message_to_parked_agent_without_reason_buffers_with_warning() {
    let state = make_state();
    install_inbox_store(&state).await;
    let root = AgentId::random();
    let parked = AgentId::random();
    {
        let mut reg = state.registry.write().await;
        add_root(&mut reg, &root);
        add_sub(&mut reg, &parked, &root);
        let live = reg.get(&parked).unwrap().as_live().unwrap();
        live.agent.state.store(
            crate::state::AgentState::PARKED,
            std::sync::atomic::Ordering::Relaxed,
        );
    }
    let (prompt_tx, mut prompt_rx) = tokio::sync::mpsc::channel::<String>(4);
    {
        let mut reg = state.registry.write().await;
        let live = reg.get_mut(&parked).unwrap().as_live_mut().unwrap();
        live.agent.prompt_tx = prompt_tx;
    }
    let resp = send_message(
        State(state.clone()),
        AuthIdentity::test_new(Identity::Operator),
        Path(parked.clone()),
        Json(MessageRequest { text: "hi".into() }),
    )
    .await
    .expect("invariant break must not fail the send");
    let (_status, Json(body)) = resp;
    assert!(
        body.warning
            .as_deref()
            .unwrap_or("")
            .contains("invariant break"),
        "the warning must name the invariant break: {:?}",
        body.warning
    );
    assert_eq!(state.inboxes.get().unwrap().len_for(&parked).await, 1);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), prompt_rx.recv())
            .await
            .is_err(),
        "no kick turn may be fabricated without a parked reason"
    );
}

// -- Duty gate: off-duty messages buffer to inbox --

/// An off-duty agent buffers messages to its inbox instead of delivering
/// to the prompt channel. The message never reaches `prompt_rx`.
#[tokio::test]
async fn off_duty_message_buffers_to_inbox() {
    let state = make_state();
    install_inbox_store(&state).await;
    let receiver = AgentId::random();
    let (mut entry, mut rx) = make_entry_with_rx(None, "recv".into());
    entry.identity.config.role = "root".into();
    state
        .registry
        .write()
        .await
        .register(receiver.clone(), RegistryEntry::Live(entry));
    // Set the agent off-duty.
    state
        .duty
        .set(receiver.clone(), crate::duty::DutyStatus::OffDuty);

    let resp = send_message(
        State(state.clone()),
        AuthIdentity::test_new(Identity::Operator),
        Path(receiver.clone()),
        Json(MessageRequest {
            text: "urgent task".into(),
        }),
    )
    .await
    .expect("off-duty send should still return accepted");
    assert_eq!(resp.0, StatusCode::ACCEPTED);
    // Warning mentions off-duty buffering.
    assert!(
        resp.1.warning.is_some(),
        "off-duty response should carry a warning"
    );
    assert!(
        resp.1.warning.as_ref().unwrap().contains("off-duty"),
        "warning should mention off-duty: {:?}",
        resp.1.warning
    );
    // Message was NOT delivered to the prompt channel.
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv())
            .await
            .is_err(),
        "no message should be delivered to an off-duty agent"
    );
    // Message IS in the inbox.
    assert_eq!(state.inboxes.get().unwrap().len_for(&receiver).await, 1);
}

/// An on-duty agent receives messages normally (no buffering).
#[tokio::test]
async fn on_duty_message_stored_in_inbox() {
    let state = make_state();
    install_inbox_store(&state).await;
    let receiver = AgentId::random();
    let (mut entry, _rx) = make_entry_with_rx(None, "recv".into());
    entry.identity.config.role = "root".into();
    state
        .registry
        .write()
        .await
        .register(receiver.clone(), RegistryEntry::Live(entry));

    let resp = send_message(
        State(state.clone()),
        AuthIdentity::test_new(Identity::Operator),
        Path(receiver.clone()),
        Json(MessageRequest {
            text: "hello".into(),
        }),
    )
    .await
    .expect("on-duty send accepted");
    assert_eq!(resp.0, StatusCode::ACCEPTED);
    assert!(resp.1.warning.is_none(), "on-duty should have no warning");

    // On-duty message is stored in the inbox.
    let msg = state
        .inboxes
        .get()
        .unwrap()
        .pull_undelivered(&receiver)
        .await
        .unwrap();
    assert!(msg.contains("hello"));
}

/// Off-duty messages buffer to inbox; on-duty messages also go to inbox.
/// The agent task loop pulls on notify wake.
#[tokio::test]
async fn duty_toggle_off_then_on() {
    let state = make_state();
    install_inbox_store(&state).await;
    let receiver = AgentId::random();
    let (mut entry, _rx) = make_entry_with_rx(None, "recv".into());
    entry.identity.config.role = "root".into();
    state
        .registry
        .write()
        .await
        .register(receiver.clone(), RegistryEntry::Live(entry));

    // Off-duty: message buffered to inbox.
    state
        .duty
        .set(receiver.clone(), crate::duty::DutyStatus::OffDuty);
    let _ = send_message(
        State(state.clone()),
        AuthIdentity::test_new(Identity::Operator),
        Path(receiver.clone()),
        Json(MessageRequest {
            text: "first".into(),
        }),
    )
    .await
    .unwrap();
    assert_eq!(state.inboxes.get().unwrap().len_for(&receiver).await, 1);

    // Back on-duty: message also goes to inbox.
    state
        .duty
        .set(receiver.clone(), crate::duty::DutyStatus::OnDuty);
    let _ = send_message(
        State(state.clone()),
        AuthIdentity::test_new(Identity::Operator),
        Path(receiver.clone()),
        Json(MessageRequest {
            text: "second".into(),
        }),
    )
    .await
    .unwrap();
    assert_eq!(state.inboxes.get().unwrap().len_for(&receiver).await, 2);

    // Pull both messages.
    let msg = state
        .inboxes
        .get()
        .unwrap()
        .pull_undelivered(&receiver)
        .await
        .unwrap();
    assert!(msg.contains("first"));
    assert!(msg.contains("second"));
}

// -- inbound recording vs refusal: a refused message must not append a
// transcript row (the frontend re-sends refusals), an accepted one records
// exactly one, and an accepted-but-buffered (off-duty) one still records --

use crate::external::ExternalProjector;
use crate::relay::MessageLimits;
use kallip_agora_common::ids::{ConversationId, TagmaId};
use std::sync::Arc;
use tempfile::TempDir;

/// Install an external projector with a real (tempdir) history store and
/// subscribe to its frames. Returns the db handle, the frame receiver, and
/// the tempdir (keep it alive for the test's duration).
async fn install_projector(
    state: &crate::state::SharedState,
) -> (
    crate::relay::chat_history::Db,
    tokio::sync::broadcast::Receiver<crate::external::ExternalFrame>,
    TempDir,
) {
    let dir = TempDir::new().unwrap();
    let db = crate::relay::chat_history::open(&dir.path().join("h.sqlite"))
        .await
        .unwrap();
    let projector = ExternalProjector::new(
        Arc::downgrade(state),
        Some(db.clone()),
        Some(ConversationId::for_tagma(&TagmaId::from("t".to_string()))),
        Some(TagmaId::from("t".to_string())),
        Some("Tagma".into()),
        MessageLimits::default(),
    );
    let rx = projector.subscribe();
    let _ = state.external.set(projector);
    (db, rx, dir)
}

async fn operator_rows(
    db: &crate::relay::chat_history::Db,
) -> Vec<crate::relay::chat_history::HistoryRow> {
    crate::relay::chat_history::read_last_n(db, None, 10)
        .await
        .unwrap()
}

/// A parked send is accepted (auto-wake): exactly one transcript row and
/// one UserMessage frame — recording follows acceptance, and the kick turn
/// itself (a system prompt, not an operator message) records nothing.
#[tokio::test]
async fn parked_send_records_row_and_frame_exactly_once() {
    let state = make_state();
    install_inbox_store(&state).await;
    let (db, mut rx, _dir) = install_projector(&state).await;
    let root = AgentId::random();
    let (entry, _rx) = make_entry_with_rx(None, "root".into());
    {
        let mut reg = state.registry.write().await;
        reg.register_root(root.clone(), RegistryEntry::Live(entry))
            .unwrap();
        let live = reg.get(&root).unwrap().as_live().unwrap();
        live.agent.state.store(
            crate::state::AgentState::PARKED,
            std::sync::atomic::Ordering::Relaxed,
        );
        *live.agent.parked.lock().unwrap() = Some(crate::state::ParkedSnapshot {
            reason: kallip_common::protocol::ParkedReason::MaxRoundsExceeded,
            at: std::time::Instant::now(),
        });
    }

    let (status, _body) = send_message(
        State(state.clone()),
        AuthIdentity::test_new(Identity::Operator),
        Path(root.clone()),
        Json(MessageRequest { text: "hi".into() }),
    )
    .await
    .expect("parked root accepts the message (auto-wake)");
    assert_eq!(status, StatusCode::ACCEPTED);

    assert_eq!(
        operator_rows(&db).await.len(),
        1,
        "an accepted parked send records exactly one transcript row"
    );
    assert!(
        rx.try_recv().is_ok(),
        "the accepted send publishes its UserMessage frame"
    );
    assert!(
        rx.try_recv().is_err(),
        "no second frame: the kick turn records nothing"
    );
    assert_eq!(state.inboxes.get().unwrap().len_for(&root).await, 1);
}

/// Repeated parked sends each record exactly one row — there is no client
/// wake-and-retry dance anymore (the server auto-wakes), so the pile-up
/// hazard the old refusal test guarded against cannot arise; the pin is
/// one row per accepted send, no duplicates.
#[tokio::test]
async fn repeated_parked_sends_each_record_one_row() {
    let state = make_state();
    install_inbox_store(&state).await;
    let (db, mut rx, _dir) = install_projector(&state).await;
    let root = AgentId::random();
    let (entry, _rx) = make_entry_with_rx(None, "root".into());
    {
        let mut reg = state.registry.write().await;
        reg.register_root(root.clone(), RegistryEntry::Live(entry))
            .unwrap();
        let live = reg.get(&root).unwrap().as_live().unwrap();
        live.agent.state.store(
            crate::state::AgentState::PARKED,
            std::sync::atomic::Ordering::Relaxed,
        );
        *live.agent.parked.lock().unwrap() = Some(crate::state::ParkedSnapshot {
            reason: kallip_common::protocol::ParkedReason::MaxRoundsExceeded,
            at: std::time::Instant::now(),
        });
    }

    for i in 0..2 {
        let (status, body) = send_message(
            State(state.clone()),
            AuthIdentity::test_new(Identity::Operator),
            Path(root.clone()),
            Json(MessageRequest {
                text: format!("hi {i}"),
            }),
        )
        .await
        .expect("every parked send is accepted (auto-wake)");
        assert_eq!(status, StatusCode::ACCEPTED);
        assert!(
            body.warning.as_deref().unwrap_or("").contains("kick turn"),
            "each send auto-wakes: {:?}",
            body.warning
        );
    }

    assert_eq!(
        operator_rows(&db).await.len(),
        2,
        "two accepted sends record exactly two rows — no duplicates, no gaps"
    );
    assert!(rx.try_recv().is_ok() && rx.try_recv().is_ok());
    assert!(
        rx.try_recv().is_err(),
        "no extra frames beyond the two sends"
    );
    assert_eq!(state.inboxes.get().unwrap().len_for(&root).await, 2);
}

/// An off-duty agent accepts-and-buffers; the inbound row must still be
/// recorded (recording follows acceptance, not delivery to the agent).
#[tokio::test]
async fn off_duty_message_still_recorded() {
    let state = make_state();
    install_inbox_store(&state).await;
    let (db, _rx, _dir) = install_projector(&state).await;
    let root = AgentId::random();
    let (entry, _rx) = make_entry_with_rx(None, "root".into());
    {
        let mut reg = state.registry.write().await;
        reg.register_root(root.clone(), RegistryEntry::Live(entry))
            .unwrap();
    }
    state
        .duty
        .set(root.clone(), crate::duty::DutyStatus::OffDuty);

    let resp = send_message(
        State(state.clone()),
        AuthIdentity::test_new(Identity::Operator),
        Path(root),
        Json(MessageRequest {
            text: "later".into(),
        }),
    )
    .await
    .expect("off-duty send is accepted (buffered)");
    assert_eq!(resp.0, StatusCode::ACCEPTED);

    let rows = operator_rows(&db).await;
    assert_eq!(rows.len(), 1, "buffered-but-accepted message still records");
}

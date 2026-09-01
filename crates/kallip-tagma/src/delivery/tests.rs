// Direct tests for enqueue_prompt's slow path (reactivation). Historically
// every test held the prompt receiver open to stay on the notify fast path,
// because falling through spawns a real runtime; `AppState::spawn_fn` lets
// these tests stub the spawn and observe the reactivation semantics: the
// message buffers to the inbox (no channel pre-send), the spawn receives the
// dead incarnation's store/approvals/env, and the fresh agent is written back
// live. The reserve-step handle aborts are not asserted (JoinHandle has no
// comparable identity once swapped) -- they run in the same locked block that
// installs the fresh channel.

use std::sync::{Arc, Mutex};

use kallip_common::agentid::AgentId;

use crate::lifecycle::SpawnArgs;
use crate::state::{AgentEntry, RegistryEntry, SharedState};
use crate::test_helpers::{
    install_inbox_store, make_entry_with_rx, make_state, make_state_with_spawn,
};

#[derive(Default)]
struct Seen {
    store: Mutex<Option<Arc<tokio::sync::Mutex<kallip_runtime::context::ContextStore>>>>,
    approvals: Mutex<Option<Arc<tokio::sync::Mutex<kallip_runtime::approval::ApprovalStore>>>>,
    initial_prompt: Mutex<Option<String>>,
}

fn state_with_stub(seen: Arc<Seen>, fail: bool) -> SharedState {
    make_state_with_spawn(Arc::new(move |args: SpawnArgs| {
        *seen.store.lock().unwrap() = Some(args.store.clone());
        *seen.approvals.lock().unwrap() = Some(args.approvals.clone());
        *seen.initial_prompt.lock().unwrap() = args.initial_prompt.clone();
        let fresh = make_entry_with_rx(None, "agent-stub".to_string());
        let (agent, identity) = {
            let AgentEntry {
                identity, agent, ..
            } = fresh.0;
            (agent, identity)
        };
        let _keep_rx_open = fresh.1;
        Box::pin(async move {
            if fail {
                anyhow::bail!("stub spawn failure");
            }
            Ok((agent, identity))
        })
    }))
}

/// Register a live entry whose prompt receiver is dropped: the channel reads
/// closed, so enqueue_prompt falls through to reactivation instead of
/// notifying. Returns the dead incarnation's store for identity preservation
/// assertions.
async fn register_dead_root(
    state: &SharedState,
) -> (
    AgentId,
    Arc<tokio::sync::Mutex<kallip_runtime::context::ContextStore>>,
) {
    let id = AgentId::random();
    let (entry, _rx) = make_entry_with_rx(None, format!("agent-{id}"));
    let store = entry.agent.store.clone();
    drop(_rx);
    state
        .registry
        .write()
        .await
        .register(id.clone(), RegistryEntry::Live(entry));
    state.duty.set(id.clone(), crate::duty::DutyStatus::OnDuty);
    (id, store)
}

#[tokio::test]
async fn slow_path_buffers_to_inbox_passes_identity_and_reinstalls_live() {
    let seen = Arc::new(Seen::default());
    let state = state_with_stub(seen.clone(), false);
    install_inbox_store(&state).await;
    let (id, dead_store) = register_dead_root(&state).await;

    let resp = crate::delivery::enqueue_prompt(&state, &id, "hello".to_string(), "operator")
        .await
        .expect("slow path succeeds with stubbed spawn");
    assert_eq!(resp.queue_depth, 0);
    assert!(resp.warning.is_none());

    // Spawn received the dead incarnation's store (preserved identity) and no
    // pre-sent prompt (the message rides the inbox instead).
    let seen_store = seen.store.lock().unwrap().clone().expect("spawn called");
    assert!(
        Arc::ptr_eq(&seen_store, &dead_store),
        "SpawnArgs.store preserved"
    );
    assert!(
        seen.initial_prompt.lock().unwrap().is_none(),
        "no channel pre-send"
    );

    // The message is in the inbox.
    let inbox = state.inboxes.get().expect("inbox installed");
    let listed = inbox.list(&id, &crate::inbox::InboxFilter::default()).await;
    assert!(
        listed.iter().any(|e| e.body == "hello"),
        "message buffered to inbox"
    );

    // The fresh agent is written back live with an open channel.
    let prompt_tx_open = {
        let registry = state.registry.read().await;
        !registry
            .get(&id)
            .expect("agent still registered")
            .as_live()
            .expect("entry live after reactivation")
            .agent
            .prompt_tx
            .is_closed()
    };
    assert!(!prompt_tx_open, "fresh prompt channel installed");
}

#[tokio::test]
async fn slow_path_spawn_failure_leaves_agent_dead_with_conflict_free_state() {
    let seen = Arc::new(Seen::default());
    let state = state_with_stub(seen.clone(), true);
    install_inbox_store(&state).await;
    let (id, _store) = register_dead_root(&state).await;

    let err = crate::delivery::enqueue_prompt(&state, &id, "hello".to_string(), "operator")
        .await
        .expect_err("stubbed spawn failure surfaces");
    assert!(
        err.to_string().contains("500"),
        "spawn failure surfaces as an internal error: {err}"
    );

    // The message still landed in the inbox (the push precedes the gate), so a
    // later retry pulls it once the agent finally wakes.
    let inbox = state.inboxes.get().expect("inbox installed");
    let listed = inbox.list(&id, &crate::inbox::InboxFilter::default()).await;
    assert!(
        listed.iter().any(|e| e.body == "hello"),
        "message stays buffered after failed spawn"
    );
}

// -- dangling-binding gate (reject before the inbox write) --

/// A live idle agent whose recorded binding does not resolve: the send is
/// rejected and the message never reaches the inbox (both the notify fast
/// path and the kick path pass through the gate).
#[tokio::test]
async fn delivery_rejected_for_dangling_agent_live_idle() {
    let state = make_state();
    install_inbox_store(&state).await;
    let id = AgentId::random();
    let (mut entry, _rx) = make_entry_with_rx(None, format!("agent-{id}"));
    entry.identity.config.profile_set = Some("gone".into());
    state
        .registry
        .write()
        .await
        .register(id.clone(), RegistryEntry::Live(entry));
    state.duty.set(id.clone(), crate::duty::DutyStatus::OnDuty);

    let err = crate::delivery::enqueue_prompt(&state, &id, "hello".to_string(), "operator")
        .await
        .expect_err("dangling binding must reject");
    assert_eq!(err.status, 409);
    assert!(
        err.message.contains("no usable profile set"),
        "{}",
        err.message
    );

    let inbox = state.inboxes.get().expect("inbox installed");
    let listed = inbox.list(&id, &crate::inbox::InboxFilter::default()).await;
    assert!(
        listed.is_empty(),
        "rejected message must not reach the inbox"
    );
}

/// The parked kick path is gated the same way: a parked agent with an
/// unbound record is not kickable, and the message never reaches the inbox.
#[tokio::test]
async fn delivery_rejected_for_parked_kick() {
    let state = make_state();
    install_inbox_store(&state).await;
    let id = AgentId::random();
    let (mut entry, _rx) = make_entry_with_rx(None, format!("agent-{id}"));
    entry.identity.config.profile_set = None;
    entry.agent.state.store(
        crate::state::AgentState::PARKED,
        std::sync::atomic::Ordering::Relaxed,
    );
    *entry.agent.parked.lock().unwrap() = Some(crate::state::ParkedSnapshot {
        reason: kallip_common::protocol::ParkedReason::FatalError {
            message: "boom".to_string(),
        },
        at: std::time::Instant::now(),
    });
    state
        .registry
        .write()
        .await
        .register(id.clone(), RegistryEntry::Live(entry));
    state.duty.set(id.clone(), crate::duty::DutyStatus::OnDuty);

    let err = crate::delivery::enqueue_prompt(&state, &id, "hello".to_string(), "operator")
        .await
        .expect_err("unbound parked agent must not be kickable");
    assert_eq!(err.status, 409);

    let inbox = state.inboxes.get().expect("inbox installed");
    let listed = inbox.list(&id, &crate::inbox::InboxFilter::default()).await;
    assert!(
        listed.is_empty(),
        "rejected message must not reach the inbox"
    );
}

/// The delivery fast path observes lock visibility (F2-B): a live
/// Normal-class agent missing its workspace lock logs one WARN — and stays
/// silent once the lock is back. Log-only: the delivery itself is unaffected.
#[tokio::test]
async fn delivery_fast_path_warns_when_workspace_lock_is_missing() {
    use tracing_subscriber::prelude::*;

    struct LogBuf(Arc<Mutex<Vec<u8>>>);
    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuf {
        type Writer = LogGuard<'a>;
        fn make_writer(&'a self) -> Self::Writer {
            LogGuard(self.0.lock().unwrap())
        }
    }
    struct LogGuard<'a>(std::sync::MutexGuard<'a, Vec<u8>>);
    impl std::io::Write for LogGuard<'_> {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.write(buf)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.0.flush()
        }
    }

    let shared = Arc::new(Mutex::new(Vec::<u8>::new()));
    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(LogBuf(shared.clone()))
            .with_filter(tracing_subscriber::EnvFilter::new("warn")),
    );
    let _guard = tracing::subscriber::set_default(subscriber);

    let state = make_state();
    install_inbox_store(&state).await;
    let tmp = tempfile::TempDir::new_in("/dev/shm").unwrap();
    let ws = tmp.path().join("f2b-warn");
    std::fs::create_dir_all(&ws).unwrap();
    let id = AgentId::random();
    let (mut entry, _rx) = make_entry_with_rx(None, format!("agent-{id}"));
    entry.identity.config.workspace_root = ws.clone();
    state
        .registry
        .write()
        .await
        .register(id.clone(), RegistryEntry::Live(entry));
    state.duty.set(id.clone(), crate::duty::DutyStatus::OnDuty);

    // Lock missing: the WARN fires and the delivery still succeeds.
    let response = crate::delivery::enqueue_prompt(&state, &id, "hello".to_string(), "operator")
        .await
        .expect("delivery must succeed; the probe is log-only");
    assert!(
        response.warning.is_none(),
        "a log-only probe never surfaces on the wire"
    );
    let captured = String::from_utf8(shared.lock().unwrap().clone()).expect("log bytes are utf8");
    assert!(
        captured.contains("missing its workspace lock"),
        "expected the lock-evaporation WARN, got: {captured}"
    );
    assert!(
        captured.contains(&id.to_string()),
        "the WARN must carry the agent id"
    );

    // Lock restored: the same delivery is silent.
    state.lock_manager.acquire(&id, &ws, &[]).unwrap();
    shared.lock().unwrap().clear();
    crate::delivery::enqueue_prompt(&state, &id, "again".to_string(), "operator")
        .await
        .expect("second delivery succeeds");
    let captured = String::from_utf8(shared.lock().unwrap().clone()).expect("log bytes are utf8");
    assert!(
        !captured.contains("missing its workspace lock"),
        "a held lock must not warn: {captured}"
    );
}

//! The status pump: snapshots the tagma's aggregate runtime state (agent
//! counts + token budget) on a fixed cadence and posts it to the lesche, which
//! rebroadcasts it as a `LescheEvent::TagmaStatus` on the owner's app event
//! stream. Unlike the event [`pump`](super::pump), it is bounded to the tunnel
//! session (not the KEX epoch): status is plaintext and key-independent, so it
//! needs none of the pump's drain-before-rotation semantics.
//!
//! Status is a periodic full snapshot, not a delta log: a dropped frame just
//! means slightly-stale data until the next tick, so there is no sequence
//! tracking and no replay path.
//! An unchanged snapshot is not re-posted until FORCED_RESEND (30s) has
//! elapsed since the last successful POST: the periodic full snapshot
//! doubles as a catch-up heartbeat, because lesche drops a POST with no
//! live owner stream and still returns 202 -- plain suppression would
//! starve subscribers that connect after the last change.

use std::time::Duration;

use kallip_common::protocol::AgentState;
use kallip_lesche_common::event::TagmaStatusPayload;
use kallip_runtime::token_budget::TokenBudget;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::RelayHandle;
use crate::state::AgentRegistry;

/// The snapshot cadence. Source-tunable; not exposed as an env knob until an
/// operator asks for it.
const STATUS_INTERVAL: Duration = Duration::from_secs(2);
/// How long an unchanged snapshot may stay silent. Upper bound on the
/// catch-up window for a subscriber that connects after the last change:
/// lesche acknowledges a POST even when it drops it (no live owner
/// stream), so suppression alone would never re-arm a late subscriber.
const FORCED_RESEND: Duration = Duration::from_secs(30);

/// The suppression decision, factored out pure: post when nothing has
/// been posted yet, when the payload changed, or when the last successful
/// POST is older than FORCED_RESEND (the catch-up heartbeat).
fn needs_post(
    last: Option<(&TagmaStatusPayload, std::time::Instant)>,
    payload: &TagmaStatusPayload,
    now: std::time::Instant,
) -> bool {
    match last {
        None => true,
        Some((posted, at)) => posted != payload || now.duration_since(at) >= FORCED_RESEND,
    }
}

/// Build a status payload from the current registry + token budget. Pure
/// (lock-free) so it can be unit-tested in isolation. Partitions agents into
/// the root (the conversation peer) vs subagents (spawned helpers) in a single
/// pass via `AgentConfig::is_root` (= `created_by == None`). State goes
/// through `RegistryEntry::state_for_summary` so Live and Faulted entries are
/// handled uniformly (Faulted entries have no bridge-owned atomic).
///
/// `root_state` defaults to [`AgentState::Faulted`] as a defensive fallback
/// for the (production-unreachable) case where no root entry is registered
/// when the pump ticks -- the root is created at startup and non-removable
/// while live, so this only fires under a logic bug. `Faulted` is the safe
/// "no live peer" signal either way.
pub(crate) fn snapshot_status(
    registry: &AgentRegistry,
    token_budget: &TokenBudget,
) -> TagmaStatusPayload {
    let mut root_state = AgentState::Faulted;
    let mut sub_total = 0u32;
    let mut sub_active = 0u32;
    for (_, entry) in registry.iter() {
        let state = entry.state_for_summary();
        if entry.identity().config.is_root() {
            root_state = state;
        } else {
            sub_total += 1;
            if state == AgentState::Busy {
                sub_active += 1;
            }
        }
    }
    let budget = token_budget.snapshot();
    TagmaStatusPayload {
        root_state,
        subagents_total: sub_total,
        subagents_active: sub_active,
        token_budget: budget.budget,
        token_consumed: budget.consumed,
    }
}

impl RelayHandle {
    /// Ensure the status pump is running. Idempotent: a no-op if one is
    /// already live. Started on tunnel-up; stopped on tunnel-down so a
    /// reconnect installs a fresh pump against the new session.
    pub(super) async fn start_status_pump(&self) {
        let mut slot = self.inner.status_pump.lock().await;
        if slot.is_some() {
            return;
        }
        let cancel = CancellationToken::new();
        let task = tokio::spawn(self.clone().run_status_pump(cancel.clone()));
        *slot = Some(super::PumpHandle { task, cancel });
    }

    /// Stop and await the status pump if it is running, clearing the slot so a
    /// later `start_status_pump` can install a fresh one.
    pub(super) async fn stop_status_pump(&self) {
        let handle = { self.inner.status_pump.lock().await.take() };
        if let Some(handle) = handle {
            handle.cancel.cancel();
            let _ = handle.task.await;
        }
    }

    /// Snapshot total/active agent counts + the token budget and POST them
    /// every STATUS_INTERVAL until cancel fires; an unchanged snapshot is skipped.
    ///
    /// `Weak::upgrade() == None` (AppState dropped) ends the loop: the tagma is
    /// shutting down. The registry read-guard is dropped before the POST so no
    /// `.await` is held under the lock (lock discipline invariant #1);
    /// `state_for_summary` and `token_budget.snapshot` are lock-free (atomics).
    async fn run_status_pump(self, cancel: CancellationToken) {
        info!(tagma = %self.inner.tagma_id, "relay status pump started");
        let mut ticker = tokio::time::interval(STATUS_INTERVAL);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        // Last successfully posted snapshot + its timestamp: a tick whose
        // payload equals it is skipped until FORCED_RESEND elapses (the
        // catch-up heartbeat for late subscribers), and a failed POST keeps
        // the old value so the next tick retries. Updated only on success.
        let mut last_posted: Option<(TagmaStatusPayload, std::time::Instant)> = None;
        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    info!(tagma = %self.inner.tagma_id, "relay status pump stopped");
                    return;
                }
                _ = ticker.tick() => {}
            }
            let Some(state) = self.inner.state.upgrade() else {
                // AppState dropped: the tagma is shutting down.
                return;
            };
            // Snapshot under the registry read-lock, then drop the guard before
            // the POST. `state_for_summary` reads only the bridge-owned atomic
            // (Live) or returns a constant (Faulted) -- no per-agent lock.
            let payload = snapshot_status(&*state.registry.read().await, &state.token_budget);
            let now = std::time::Instant::now();
            let last = last_posted.as_ref().map(|(p, at)| (p, *at));
            if !needs_post(last, &payload, now) {
                continue;
            }
            // No retry: status is idempotent and the next tick supersedes a
            // dropped POST, so retrying would only amplify a transient stall.
            // The POST is cancel-select'd so a tunnel-down (cancel fires) aborts
            // it immediately instead of waiting out the 30s HTTP timeout --
            // mirroring `emit` (mod.rs). Without this, `stop_status_pump` could
            // stall and the POST would outlive the tunnel it is bounded to.
            let post = self
                .inner
                .client
                .post_status(&self.inner.tagma_id, &payload);
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return,
                r = post => {
                    match r {
                        Ok(()) => {
                            last_posted = Some((payload, std::time::Instant::now()));
                        }
                        Err(e) => {
                            warn!(tagma = %self.inner.tagma_id, "status post failed: {e:#}");
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{RegistryEntry, SharedState};
    use crate::test_helpers::{
        add_faulted_root, add_faulted_sub, add_root, add_sub, make_entry, make_state,
    };
    use kallip_common::agentid::AgentId;
    use kallip_e2ee::DeviceKey;
    use kallip_lesche_client::LescheClient;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use tokio::sync::Mutex;

    /// Captured status POST bodies, in arrival order.
    type Capture = Arc<Mutex<Vec<TagmaStatusPayload>>>;

    /// Spawn a mock lesche that captures `POST /v1/tagmata/{tagma}/status` and
    /// returns 202. Mirrors the op_tests mock-lesche pattern (no wiremock dep).
    async fn spawn_status_lesche(capture: Capture) -> String {
        async fn handler(
            axum::extract::State(c): axum::extract::State<Capture>,
            axum::Json(payload): axum::Json<TagmaStatusPayload>,
        ) -> &'static str {
            c.lock().await.push(payload);
            "ok"
        }
        let app = axum::Router::new()
            .route("/v1/tagmata/{tagma}/status", axum::routing::post(handler))
            .with_state(capture);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    /// Spawn a mock lesche whose first status POST fails with a 500 (not
    /// captured) and every later one succeeds (captured). Returns the url;
    /// the attempt counter is shared with the caller.
    async fn spawn_flaky_status_lesche(capture: Capture, attempts: Arc<AtomicUsize>) -> String {
        async fn handler(
            axum::extract::State((c, n)): axum::extract::State<(Capture, Arc<AtomicUsize>)>,
            axum::Json(payload): axum::Json<TagmaStatusPayload>,
        ) -> (axum::http::StatusCode, &'static str) {
            if n.fetch_add(1, Ordering::SeqCst) == 0 {
                return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "boom");
            }
            c.lock().await.push(payload);
            (axum::http::StatusCode::OK, "ok")
        }
        let app = axum::Router::new()
            .route("/v1/tagmata/{tagma}/status", axum::routing::post(handler))
            .with_state((capture, attempts));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    /// Build a RelayHandle wired to a capturing mock lesche + a real AppState
    /// whose single root agent makes `root_state != Faulted` (i.e. the snapshot
    /// reflects a real root, not the zero-root fallback). Returns the handle,
    /// the capture, and the AppState strong ref (the pump only holds a Weak, so
    /// the caller must keep it alive for the pump's `upgrade()` to resolve).
    async fn setup_pump_at(capture: Capture, url: String) -> (RelayHandle, Capture, SharedState) {
        let state = make_state();
        let root = AgentId::from("root".to_string());
        {
            let mut registry = state.registry.write().await;
            registry
                .register_root(
                    root.clone(),
                    RegistryEntry::Live(make_entry(None, "tok".into())),
                )
                .expect("register root");
        }
        let client = LescheClient::builder(&url, "tok").build().unwrap();
        let handle = RelayHandle::new(
            client,
            "test".to_string(),
            kallip_archeion_common::ids::TagmaId::from("tagma".to_string()),
            "Tagma".into(),
            DeviceKey::generate(),
            root,
            Arc::downgrade(&state),
        );
        (handle, capture, state)
    }

    async fn setup_pump() -> (RelayHandle, Capture, SharedState) {
        let capture: Capture = Arc::new(Mutex::new(Vec::new()));
        let url = spawn_status_lesche(capture.clone()).await;
        setup_pump_at(capture, url).await
    }

    /// The pump POSTs a snapshot on its first tick (tokio interval fires at
    /// t=0), then `stop_status_pump` cleanly reaps the task.
    #[tokio::test]
    async fn status_pump_posts_snapshot_on_first_tick() {
        let (handle, capture, _state) = setup_pump().await;
        handle.start_status_pump().await;
        // The first tick is immediate; poll for up to 500ms.
        let mut got = Vec::new();
        for _ in 0..50 {
            got = capture.lock().await.clone();
            if !got.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        handle.stop_status_pump().await;
        assert_eq!(got.len(), 1, "exactly one snapshot on the first tick");
        assert_eq!(got[0].root_state, AgentState::Idle, "root is idle");
        assert_eq!(
            (got[0].subagents_total, got[0].subagents_active),
            (0, 0),
            "no subagents"
        );
    }

    /// The root is reported separately from subagents. `subagents_total`
    /// counts Live + Faulted subs (matching `list_agents`); `subagents_active`
    /// counts only subs whose `state_for_summary` is `Busy`. `root_state`
    /// tracks the root's own state. Guards against regressing to a raw-atomic
    /// read, which would panic on Faulted entries (they have no atomic).
    #[tokio::test]
    async fn snapshot_partitions_root_from_subagents() {
        let state = make_state();
        let root = AgentId::from("root".to_string());
        let busy_sub = AgentId::from("busy".to_string());
        let idle_sub = AgentId::from("idle".to_string());
        let faulted_sub = AgentId::from("faulted".to_string());
        {
            let mut registry = state.registry.write().await;
            add_root(&mut registry, &root); // idle by default
            add_sub(&mut registry, &busy_sub, &root);
            add_sub(&mut registry, &idle_sub, &root);
            add_faulted_sub(&mut registry, &faulted_sub, &root, "boom");
            // Flip the root and `busy_sub` to BUSY via their bridge-owned
            // atomics.
            for id in [&root, &busy_sub] {
                registry
                    .get(id)
                    .expect("entry present")
                    .as_live()
                    .expect("live")
                    .agent
                    .state
                    .store(AgentState::BUSY, Ordering::Relaxed);
            }
        }
        let registry = state.registry.read().await;
        let payload = snapshot_status(&registry, &state.token_budget);
        assert_eq!(payload.root_state, AgentState::Busy);
        // busy + idle + faulted = 3 subs; only `busy_sub` is active.
        assert_eq!((payload.subagents_total, payload.subagents_active), (3, 1));
    }

    /// A faulted ROOT with live subs -- the exact disambiguation case the
    /// root/sub split exists for ("root is down but helpers are still
    /// running"): `root_state == Faulted` while subs still count normally.
    #[tokio::test]
    async fn snapshot_faulted_root_with_live_subs() {
        let state = make_state();
        let root = AgentId::from("root".to_string());
        let busy_sub = AgentId::from("busy".to_string());
        {
            let mut registry = state.registry.write().await;
            add_faulted_root(&mut registry, &root, "kaboom");
            add_sub(&mut registry, &busy_sub, &root);
            registry
                .get(&busy_sub)
                .expect("busy_sub present")
                .as_live()
                .expect("busy_sub live")
                .agent
                .state
                .store(AgentState::BUSY, Ordering::Relaxed);
        }
        let registry = state.registry.read().await;
        let payload = snapshot_status(&registry, &state.token_budget);
        assert_eq!(payload.root_state, AgentState::Faulted);
        assert_eq!((payload.subagents_total, payload.subagents_active), (1, 1));
    }

    /// An empty registry (the transient zero-root window) reports
    /// `root_state = Faulted` and zero subs, not a panic.
    #[tokio::test]
    async fn snapshot_empty_registry_reports_faulted_root() {
        let state = make_state();
        let registry = state.registry.read().await;
        let payload = snapshot_status(&registry, &state.token_budget);
        assert_eq!(payload.root_state, AgentState::Faulted);
        assert_eq!((payload.subagents_total, payload.subagents_active), (0, 0));
    }
    /// Change suppression: with an idle registry the second tick's identical
    /// snapshot is skipped, so the window captures exactly one POST. Before
    /// suppression this captured one frame per tick (two here).
    #[tokio::test]
    async fn status_pump_skips_unchanged_snapshot_on_second_tick() {
        let (handle, capture, _state) = setup_pump().await;
        handle.start_status_pump().await;
        // Two ticks (immediate + 2s) plus slack for scheduler delay.
        tokio::time::sleep(Duration::from_millis(2500)).await;
        handle.stop_status_pump().await;
        assert_eq!(
            capture.lock().await.len(),
            1,
            "identical second tick must be skipped"
        );
    }

    /// A registry change alters the payload, so the next tick posts again --
    /// the flip-to-visible contract (<= 2s) is what the suppression must keep.
    #[tokio::test]
    async fn status_pump_posts_again_after_a_change() {
        let (handle, capture, state) = setup_pump().await;
        handle.start_status_pump().await;
        // The first tick is immediate; wait for it to land.
        for _ in 0..50 {
            if !capture.lock().await.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        {
            let registry = state.registry.read().await;
            registry
                .get(&AgentId::from("root".to_string()))
                .expect("root present")
                .as_live()
                .expect("live")
                .agent
                .state
                .store(AgentState::BUSY, Ordering::Relaxed);
        }
        tokio::time::sleep(Duration::from_millis(2500)).await;
        handle.stop_status_pump().await;
        assert_eq!(capture.lock().await.len(), 2, "changed snapshot must post");
    }

    /// Unit test for the suppression decision: only a fresh (within
    /// FORCED_RESEND) unchanged payload suppresses the POST.
    #[test]
    fn needs_post_suppresses_only_fresh_unchanged_snapshots() {
        let payload = TagmaStatusPayload {
            root_state: AgentState::Idle,
            subagents_total: 0,
            subagents_active: 0,
            token_budget: 1000,
            token_consumed: 0,
        };
        let now = std::time::Instant::now();
        assert!(needs_post(None, &payload, now), "nothing posted yet");
        let fresh = Some((&payload, now - Duration::from_secs(5)));
        assert!(!needs_post(fresh, &payload, now), "fresh duplicate");
        let stale = Some((&payload, now - Duration::from_secs(31)));
        assert!(needs_post(stale, &payload, now), "stale heartbeat");
        let changed = TagmaStatusPayload {
            token_consumed: 1,
            ..payload.clone()
        };
        assert!(
            needs_post(Some((&payload, now)), &changed, now),
            "changed payload"
        );
    }

    /// A failed POST keeps the old suppression memory, so the next tick
    /// retries even though the payload is unchanged: two attempts in the
    /// two-tick window, exactly one captured (successful).
    #[tokio::test]
    async fn status_pump_retries_after_a_failed_post() {
        let capture: Capture = Arc::new(Mutex::new(Vec::new()));
        let attempts = Arc::new(AtomicUsize::new(0));
        let url = spawn_flaky_status_lesche(capture.clone(), attempts.clone()).await;
        let (handle, capture, _state) = setup_pump_at(capture, url).await;
        handle.start_status_pump().await;
        tokio::time::sleep(Duration::from_millis(2500)).await;
        handle.stop_status_pump().await;
        assert_eq!(attempts.load(Ordering::SeqCst), 2, "retry happened");
        assert_eq!(capture.lock().await.len(), 1, "only the retry captured");
    }
}

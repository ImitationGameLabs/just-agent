//! The projection pump: pushes the tagma's manage-plane projection (full
//! roster + aggregate status) to the lesche whenever the state may have
//! changed and someone is actually reading it (api-redesign §9.1/§9.7).
//!
//! Three sources wake the pump:
//! - tunnel-up: an unconditional full first snapshot (the M1 self-heal — the
//!   lesche's in-memory projection dies with its process, and a reconnecting
//!   tagma must repopulate it regardless of any buffered hint, which by
//!   definition cannot have survived either side's restart);
//! - an `ExternalFrame::Signal` on the external bus (the third subscriber:
//!   turn-state transitions nudge an immediate re-snapshot, debounced);
//! - a low-frequency fallback ticker, because the Signal vocabulary only
//!   covers turn lifecycle — roster changes (spawn/remove), duty flips,
//!   budget writes, and work-schedule edits mutate the registry silently.
//!
//! Everything is suppressed while `projection_active` is false (no subscriber
//! on the lesche side), except the tunnel-up first shot. Pushes are
//! best-effort: same contract as the status pump — no retry, the next wake
//! supersedes a dropped POST.

use std::sync::atomic::Ordering;
use std::time::Duration;

use kallip_lesche_common::projection::ProjectionSnapshot;
use tokio::sync::broadcast::error::RecvError;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::RelayHandle;
use super::status_pump::snapshot_status;
use crate::external::ExternalFrame;
use crate::state::AppState;

/// Merge window after a wake-up: state mutated several times within the
/// window collapses into one push (projection semantics allow dropping
/// intermediate states, §9.2). Implementation constant, not protocol.
const PROJECTION_DEBOUNCE: Duration = Duration::from_secs(1);

/// Build the full projection snapshot from the current registry + token
/// budget + work-schedule singleton. Lock-free like `snapshot_status`: the
/// registry read-guard is dropped before any await happens (the caller
/// holds no lock across POSTs).
fn snapshot_projection(
    state: &AppState,
    registry: &crate::state::AgentRegistry,
    push_seq: u64,
    work_schedule: Option<kallip_lesche_common::projection::WorkScheduleProjection>,
) -> ProjectionSnapshot {
    ProjectionSnapshot {
        agents: registry
            .iter()
            .map(|(id, entry)| state.summarize(id, entry))
            .collect(),
        status: snapshot_status(registry, &state.token_budget),
        push_seq,
        work_schedule,
    }
}

/// Project the tagma's work-schedule singleton into its prompt-free wire
/// form (MIN1: `wake_prompt`/`final_warn_prompt` are dropped here).
fn work_schedule_projection(
    ws: &crate::work_schedule::WorkSchedule,
) -> kallip_lesche_common::projection::WorkScheduleProjection {
    use kallip_lesche_common::projection::WorkScheduleProjection;
    WorkScheduleProjection {
        id: ws.id.clone(),
        spec: serde_json::to_value(&ws.spec).expect("spec serializes"),
        pre_warn_minutes: ws.pre_warn_minutes,
        final_warn_minutes: ws.final_warn_minutes,
        status: serde_json::to_value(ws.status).expect("status serializes"),
        created_at: ws.created_at,
    }
}

impl RelayHandle {
    /// Ensure the projection pump is running. Idempotent. Started on
    /// tunnel-up (which also fires the unconditional full first shot) and on
    /// a `false -> true` subscription-hint flip; stopped on tunnel-down and
    /// on `true -> false` so an unread projection costs nothing.
    pub(super) async fn start_projection_pump(&self) {
        let mut slot = self.inner.projection_pump.lock().await;
        if slot.is_some() {
            return;
        }
        let cancel = CancellationToken::new();
        let task = tokio::spawn(self.clone().run_projection_pump(cancel.clone()));
        *slot = Some(super::PumpHandle { task, cancel });
    }

    /// Stop and await the projection pump if it is running, clearing the slot.
    pub(super) async fn stop_projection_pump(&self) {
        let handle = { self.inner.projection_pump.lock().await.take() };
        if let Some(handle) = handle {
            handle.cancel.cancel();
            let _ = handle.task.await;
        }
    }

    /// Consume a `SubscriptionHint` from the tunnel (hierarchical
    /// consumption: the hint only starts/stops the pump -- the pump itself
    /// decides, per push, whether the flag allows a POST).
    pub(super) async fn handle_projection_hint(&self, active: bool) {
        let was = self.inner.projection_active.swap(active, Ordering::Relaxed);
        if was == active {
            return; // includes the reconnect `true -> true` re-send
        }
        if active {
            self.start_projection_pump().await;
        } else {
            self.stop_projection_pump().await;
        }
    }

    /// Subscribe to the external bus (the third subscriber) and push the
    /// projection on Signal, on the fallback tick, and once unconditionally
    /// at tunnel-up. Exits when the tunnel dies (`cancel`), the AppState
    /// drops, or the bus closes.
    async fn run_projection_pump(self, cancel: CancellationToken) {
        info!(tagma = %self.inner.tagma_id, "relay projection pump started");
        let Some(state) = self.inner.state.upgrade() else {
            return; // the tagma is shutting down
        };
        let Some(projector) = state.external.get() else {
            warn!("external projector missing; projection pump idle");
            return;
        };
        let mut rx = projector.subscribe();

        // M1 self-heal: the first snapshot rides tunnel-up unconditionally
        // (hint state is irrelevant -- the lesche may have restarted and lost
        // its stored projection while the hint stayed logically `true`).
        self.push_projection(&state, &cancel).await;

        let fallback =
            Duration::from_millis(self.inner.projection_fallback_ms.load(Ordering::Relaxed));
        let mut ticker = tokio::time::interval(fallback);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    info!(tagma = %self.inner.tagma_id, "relay projection pump stopped");
                    return;
                }
                _ = ticker.tick() => {}
                r = rx.recv() => match r {
                    Ok(ExternalFrame::Signal(_)) => {}
                    Ok(_) => continue, // authored content never moves the projection
                    Err(RecvError::Lagged(_)) => {} // missed frames: re-check
                    Err(RecvError::Closed) => {
                        warn!("external bus closed; projection pump exiting");
                        return;
                    }
                },
            }
            // Merge window: everything that changes during the debounce lands
            // in the same push. The sleep is not cancel-selected on purpose --
            // 1s is well under the tunnel teardown path's patience.
            tokio::time::sleep(PROJECTION_DEBOUNCE).await;
            // Drain whatever piled up inside the window (a lagged receiver is
            // dirty by the same argument), so one push covers all of it.
            loop {
                match rx.try_recv() {
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
            if !self.inner.projection_active.load(Ordering::Relaxed) {
                continue; // nobody is reading: skip the push (§9.7 suppression)
            }
            self.push_projection(&state, &cancel).await;
        }
    }

    /// Recompute the snapshot under the registry read-lock (guard dropped
    /// before the POST) and push it best-effort, cancel-select'd so a
    /// tunnel-down aborts the in-flight POST instead of waiting out the 30s
    /// HTTP timeout (mirrors the status pump).
    async fn push_projection(&self, state: &AppState, cancel: &CancellationToken) {
        let push_seq = self
            .inner
            .projection_push_seq
            .fetch_add(1, Ordering::Relaxed)
            + 1;
        let work_schedule = match state.work_schedules.get() {
            Some(store) => match store.get_singleton().await {
                Ok(Some(ws)) => Some(work_schedule_projection(&ws)),
                Ok(None) => None,
                Err(e) => {
                    // The projection is prompt-free by construction; a store
                    // read failure degrades this push to no-schedule rather
                    // than blocking the whole snapshot.
                    warn!(error = ?e, "work-schedule read failed; pushing without it");
                    None
                }
            },
            None => None,
        };
        let snapshot = {
            let registry = state.registry.read().await;
            snapshot_projection(state, &registry, push_seq, work_schedule)
        };
        let put = self.inner.client.put_state(&self.inner.tagma_id, &snapshot);
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {}
            r = put => {
                if let Err(e) = r {
                    warn!(tagma = %self.inner.tagma_id, "projection put failed: {e:#}");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::external::ExternalProjector;
    use crate::relay::MessageLimits;
    use crate::state::RegistryEntry;
    use crate::test_helpers::{make_entry, make_state};
    use kallip_common::protocol::SignalEvent;
    use kallip_e2ee::DeviceKey;
    use kallip_lesche_client::LescheClient;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::Mutex;

    /// Captured projection POST bodies, in arrival order.
    type Capture = Arc<Mutex<Vec<ProjectionSnapshot>>>;

    /// Spawn a mock lesche that captures `POST /v1/tagmata/{tagma}/projection`
    /// and returns 200. Mirrors the status-pump mock-lesche pattern.
    async fn spawn_projection_lesche(capture: Capture) -> String {
        async fn handler(
            axum::extract::State(c): axum::extract::State<Capture>,
            axum::Json(payload): axum::Json<ProjectionSnapshot>,
        ) -> &'static str {
            c.lock().await.push(payload);
            "ok"
        }
        let app = axum::Router::new()
            .route("/v1/tagmata/{tagma}/state", axum::routing::put(handler))
            .with_state(capture);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    /// Build a RelayHandle wired to a capturing mock lesche + a real AppState
    /// (root registered, external projector installed) so the pump's bus
    /// subscription and registry snapshot both resolve. Returns the handle,
    /// the capture, and the state strong ref (the pump holds a Weak).
    async fn setup() -> (RelayHandle, Capture, crate::state::SharedState) {
        let state = make_state();
        let root = kallip_common::agentid::AgentId::from("root".to_string());
        {
            let mut registry = state.registry.write().await;
            registry
                .register_root(
                    root.clone(),
                    RegistryEntry::Live(make_entry(None, "tok".into())),
                )
                .unwrap();
        }
        let projector = ExternalProjector::new(
            Arc::downgrade(&state),
            None,
            None,
            None,
            None,
            MessageLimits::default(),
        );
        let _ = state.external.set(projector); // single install per test state
        let capture: Capture = Arc::new(Mutex::new(Vec::new()));
        let url = spawn_projection_lesche(capture.clone()).await;
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

    /// Wait until the capture holds at least `n` snapshots (or time out).
    async fn wait_for(capture: &Capture, n: usize) -> Vec<ProjectionSnapshot> {
        for _ in 0..300 {
            let got = capture.lock().await.len();
            if got >= n {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        capture.lock().await.clone()
    }

    /// M1 self-heal: tunnel-up (pump start) pushes one full snapshot
    /// unconditionally, and a stop/start cycle (the tagma's tunnel reconnect,
    /// or a full restart) pushes another one without any hint traffic.
    #[tokio::test]
    async fn tunnel_up_first_shot_and_restart_repushes() {
        let (handle, capture, _state) = setup().await;
        handle.start_projection_pump().await;
        let got = wait_for(&capture, 1).await;
        assert_eq!(got.len(), 1, "tunnel-up first shot");
        assert_eq!(got[0].push_seq, 1, "first shot is seq 1");
        assert_eq!(
            got[0].agents.len(),
            1,
            "the registered root is in the roster"
        );
        assert_eq!(
            got[0].status.root_state,
            kallip_common::protocol::AgentState::Idle
        );
        // Simulate a reconnect: tunnel-down stops the pump, tunnel-up starts
        // a fresh one whose first shot repopulates the (possibly restarted)
        // lesche's projection.
        handle.stop_projection_pump().await;
        handle.start_projection_pump().await;
        let got = wait_for(&capture, 2).await;
        assert_eq!(got.len(), 2, "restart pushes a fresh full snapshot");
        // c-Major-1: the counter is monotonic across a pump restart -- the
        // second shot continues the sequence (2) rather than replaying 1,
        // which the lesche's same-generation check would reject.
        assert_eq!(got[1].push_seq, 2, "seq continues across restart");
        handle.stop_projection_pump().await;
    }
    /// §9.7 suppression + hierarchical hint consumption: with the pump
    /// started but the last hint `false`, wakes do not push; the `false ->
    /// true` flip restarts the pump (whose first shot lands); `true -> false`
    /// stops it again.
    #[tokio::test]
    async fn hint_flip_gates_pushes() {
        let (handle, capture, _state) = setup().await;
        // Simulate "the lesche told us nobody is listening": the pump stays
        // stopped and nothing is pushed.
        handle.handle_projection_hint(false).await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(capture.lock().await.is_empty(), "false hint: no pushes");
        // The flip to `true` starts the pump, whose first shot lands.
        handle.handle_projection_hint(true).await;
        let got = wait_for(&capture, 1).await;
        assert_eq!(got.len(), 1, "true hint starts the pump + first shot");
        // And back to `false`: the pump stops, so no further pushes.
        handle.handle_projection_hint(false).await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(capture.lock().await.len(), 1, "false hint stops pushes");
        handle.stop_projection_pump().await;
    }

    /// A burst of external-bus Signals collapses into a single debounced
    /// push (the merge window), and the fallback tick keeps the projection
    /// eventually fresh even without signals.
    #[tokio::test]
    async fn signal_burst_debounces_into_one_push() {
        let (handle, capture, state) = setup().await;
        handle.handle_projection_hint(true).await;
        let got = wait_for(&capture, 1).await;
        assert_eq!(got.len(), 1, "first shot before the burst");
        let projector = state.external.get().unwrap().clone();
        for _ in 0..3 {
            projector.publish(ExternalFrame::Signal(SignalEvent::Idle));
        }
        // One debounce window later exactly one push must have landed: three
        // signals inside the 1s merge window are a single state change.
        tokio::time::sleep(Duration::from_millis(1600)).await;
        let got = capture.lock().await.clone();
        assert_eq!(got.len(), 2, "3-signal burst -> exactly one extra push");
        handle.stop_projection_pump().await;
    }

    /// Quality m-3: with a shortened fallback interval, the tick alone (no
    /// signals) drives additional pushes while the hint is active -- the
    /// coverage for registry changes that never reach the external bus
    /// (roster/duty/budget/schedule).
    #[tokio::test]
    async fn fallback_tick_drives_a_push_without_signals() {
        let (handle, capture, _state) = setup().await;
        handle
            .inner
            .projection_fallback_ms
            .store(150, Ordering::Relaxed);
        handle.handle_projection_hint(true).await;
        let got = wait_for(&capture, 1).await;
        assert_eq!(got.len(), 1, "first shot on pump start");
        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if capture.lock().await.len() >= 2 {
                break;
            }
        }
        let got = capture.lock().await.clone();
        assert_eq!(got.len(), 2, "fallback tick pushed once more");
        handle.stop_projection_pump().await;
    }

    /// q-M-2 content nail: the work-schedule projection is prompt-free by
    /// construction -- `wake_prompt`/`final_warn_prompt` never reach the
    /// wire, and the projected fields round-trip faithfully.
    #[tokio::test]
    async fn work_schedule_projection_is_prompt_free() {
        let ws: crate::work_schedule::WorkSchedule = serde_json::from_value(serde_json::json!({
            "id": "ws-1",
            "spec": { "mode": "always" },
            "pre_warn_minutes": 5,
            "final_warn_minutes": 2,
            "final_warn_prompt": "SECRET-FINAL",
            "wake_prompt": "SECRET-WAKE",
            "status": "paused",
            "created_at": "2026-01-01T00:00:00Z"
        }))
        .expect("schedule parses");
        let projected = super::work_schedule_projection(&ws);
        let json = serde_json::to_value(&projected).expect("projection serializes");
        let obj = json.as_object().expect("projection is an object");
        for forbidden in ["wake_prompt", "final_warn_prompt", "message"] {
            assert!(!obj.contains_key(forbidden), "{forbidden} must not leak");
        }
        assert_eq!(obj["id"], "ws-1");
        assert_eq!(obj["spec"]["mode"], "always");
        assert_eq!(obj["pre_warn_minutes"], 5);
        assert_eq!(obj["final_warn_minutes"], 2);
        assert_eq!(obj["status"], "paused");
    }
}

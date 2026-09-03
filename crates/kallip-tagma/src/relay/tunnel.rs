//! The lesche tunnel: the SSE stream reader + reconnect loop, and the inbound
//! dispatch fan-out.
//!
//! Extracted from `mod.rs`. A child module of `relay`, so `use super::*` reuses
//! the parent's private imports and grants access to [`RelayHandle`]'s private
//! fields/methods. The pump lifecycle methods it drives (`start/stop_pump`,
//! `start/stop_status_pump`, `start/stop_room_pump`) live in sibling child
//! modules and are already `pub(super)`.

use super::*;

impl RelayHandle {
    /// Hold the lesche tunnel open, reconnecting with a small backoff on any
    /// disconnect or error. Selects on `shutdown` (the tagma-wide parent token)
    /// so SIGINT/SIGTERM cancels the relay alongside axum and the agents. On
    /// shutdown the pump is drained (`stop_pump`) so in-flight emits complete.
    ///
    /// Cancel-safety: `connect_and_drain` and the per-op `tokio::spawn(dispatch)`
    /// are cancel-safe at every `.await` — a cancel mid-op loses the partial op,
    /// which the app retries via host-history re-pull on reconnect.
    pub async fn run(self, shutdown: CancellationToken) {
        loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => {
                    self.stop_workers().await;
                    return;
                }
                r = self.clone().connect_and_drain() => match r {
                    Ok(()) => info!("relay tunnel stream ended; reconnecting"),
                    Err(e) => warn!("relay tunnel error: {e:#}; reconnecting"),
                }
            }
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => {
                    self.stop_workers().await;
                    return;
                }
                _ = tokio::time::sleep(ops::TUNNEL_RECONNECT_BACKOFF) => {}
            }
        }
    }

    /// Drain both the pump and the in-flight op-dispatch tasks. Called from the
    /// shutdown branches of [`RelayHandle::run`].
    async fn stop_workers(&self) {
        self.stop_pump().await;
        self.stop_status_pump().await;
        self.stop_room_pump().await;
        self.stop_dispatch().await;
    }

    /// Abort and reap all in-flight op-dispatch tasks. Only safe on a process-
    /// tearing-down path: `deliver_message`'s spawn step (spawn-agent → install)
    /// is not abort-safe mid-flight, so aborting a dispatch there can leak
    /// spawned tasks or leave a disarmed workspace lock. Both `run` shutdown
    /// branches are process-exit paths, so this is acceptable; a future
    /// non-shutdown caller must first make `deliver_message` abort-safe.
    async fn stop_dispatch(&self) {
        let mut set = self.inner.dispatch.lock().await;
        set.abort_all();
        while set.join_next().await.is_some() {}
    }

    /// Open the tunnel SSE and dispatch each inbound message (each on its own
    /// task so a long-running op does not stall the stream reader). The status
    /// pump is bounded to this tunnel session: started once the tunnel is up,
    /// stopped when the stream ends so a reconnect installs a fresh pump.
    async fn connect_and_drain(self) -> Result<()> {
        let stream = self
            .inner
            .client
            .open_tunnel(&self.inner.device, &self.inner.tagma_id)
            .await?;
        self.start_status_pump().await;
        self.start_room_pump().await;
        tokio::pin!(stream);
        while let Some(item) = stream.next().await {
            match item {
                Ok(inbound) => {
                    self.inner
                        .dispatch
                        .lock()
                        .await
                        .spawn(self.clone().dispatch(inbound));
                }
                Err(e) => warn!("relay tunnel stream error: {e}"),
            }
        }
        self.stop_status_pump().await;
        self.stop_room_pump().await;
        Ok(())
    }

    async fn dispatch(self, inbound: TunnelInbound) {
        match inbound {
            TunnelInbound::KeyExchange {
                conversation_id,
                init,
            } => self.handle_kex(conversation_id, init).await,
            TunnelInbound::Envelope { envelope } => {
                // Outer last-resort: a panic that escapes the inner req_id-aware
                // boundaries in `handle_agent_op` / `handle_history`. Those cover
                // the common case (a panic yields a correlated reply/marker); a
                // panic reaching here means req_id was never parsed (or an
                // invariant broke after it), so we can only log.
                if AssertUnwindSafe(self.handle_user_op(envelope))
                    .catch_unwind()
                    .await
                    .is_err()
                {
                    error!("relay op dispatch panicked past the inner boundary");
                }
            }
            TunnelInbound::Wake => {
                // A best-effort hint that membership changed: re-poll
                // `list_my_rooms` immediately so the joined-rooms cache warms
                // before the next room envelope arrives (a just-added tagma is
                // blind until this poll lands). The Wake carries no payload, so
                // the sweep re-fetches `list_my_rooms` -- one batched GET,
                // acceptable at the expected volume.
                self.poll_rooms().await;
            }
            TunnelInbound::ManageRest {
                req_id,
                method,
                path,
                body,
            } => self.handle_manage_rest(req_id, &method, &path, body).await,
        }
    }
}

/// The frame-surface allowlist: the exact manage_router routes that may
/// ride a ManageRest frame, enumerated one by one. This is the enforcement
/// point for the encryption-scope decision: prompt-bearing routes
/// (/profiles/sets/{name}), the provider probe, session content (the lesche
/// message posts) and destructive deletes stay OFF the plaintext frame.
/// Anything not listed here is a 404, even though the underlying
/// manage_router would happily serve it -- fail-closed by construction.
fn frame_allowed(method: &str, path: &str) -> bool {
    let p = path.trim_end_matches('/');
    match (method, p) {
        ("GET", "/agents")
        | ("GET", "/budget")
        | ("POST", "/budget")
        | ("GET", "/work-schedule")
        | ("PUT", "/work-schedule")
        | ("GET", "/profiles")
        | ("POST", "/profiles/apply")
        | ("PUT", "/profiles/default") => true,
        // Per-agent subroutes: match the {id} segment explicitly.
        _ => match (
            method,
            p.strip_prefix("/agents/")
                .and_then(|rest| rest.split_once('/')),
        ) {
            ("GET", Some((_, "status"))) => true,
            ("POST", Some((_, "interrupt"))) => true,
            ("PUT", Some((_, "duty" | "metadata" | "profile-set"))) => true,
            _ => false,
        },
    }
}

impl RelayHandle {
    /// A ManageRest frame from the lesche reverse-proxy: run the frame-surface
    /// allowlist, then execute against the same manage router the envelope
    /// path uses, replying over the existing emit loop. The trace id is
    /// synthesized (frames carry no trace context).
    async fn handle_manage_rest(
        &self,
        req_id: u64,
        method: &str,
        path: &str,
        body: serde_json::Value,
    ) {
        let trace = kallip_archeion_common::ids::TraceId::from(format!("manage-rest:{req_id}"));
        if !frame_allowed(method, path) {
            warn!(
                req_id,
                method, path, "manage-rest frame denied by allowlist"
            );
            let reply = TagmaReply::ManageResult {
                req_id,
                status: 404,
                body: serde_json::json!({"error":{"message":"not on the manage frame surface"}}),
            };
            let _ = self.emit(&trace, self.agent_sender(), reply, None).await;
            return;
        }
        self.handle_manage(&trace, req_id, method, path, body).await;
    }
}

#[cfg(test)]
mod manage_rest_tests {
    use super::frame_allowed;

    #[test]
    fn allowlist_admits_low_and_medium_sensitive_routes() {
        for (method, path) in [
            ("GET", "/agents"),
            ("GET", "/agents/x/status"),
            ("POST", "/budget"),
            ("GET", "/work-schedule"),
            ("PUT", "/work-schedule"),
            ("GET", "/profiles"),
            ("PUT", "/profiles/default"),
            ("POST", "/profiles/apply"),
            ("GET", "/agents/x/status"),
            ("POST", "/agents/x/interrupt"),
            ("PUT", "/agents/x/duty"),
            ("PUT", "/agents/x/metadata"),
            ("PUT", "/agents/x/profile-set"),
        ] {
            assert!(frame_allowed(method, path), "{method} {path}");
        }
    }

    #[test]
    fn allowlist_denies_prompt_and_unlisted_routes() {
        // arch C4: prompt-bearing bodies must never ride the frame surface.
        for (method, path) in [
            ("GET", "/profiles/sets/main"),
            ("PUT", "/profiles/sets/main"),
            ("POST", "/profiles/probe"),
            // Session CONTENT riding the manage router: denied.
            ("POST", "/agents/x/lesche/messages"),
            ("POST", "/agents/x/lesche/rooms/r/messages"),
            ("POST", "/agents/x/lesche/direct-sessions/p/messages"),
            ("GET", "/agents/x/lesche/sessions"),
            ("PUT", "/budget"), // drift: the router serves POST, not PUT
            ("GET", "/agents/x/lesche/direct-sessions/p/messages"), // history read
            ("DELETE", "/profiles/sets/main"),
            ("GET", "/messages"),
            ("DELETE", "/agents/x"),
            ("GET", "/completely/unknown"),
        ] {
            assert!(!frame_allowed(method, path), "{method} {path}");
        }
    }
}

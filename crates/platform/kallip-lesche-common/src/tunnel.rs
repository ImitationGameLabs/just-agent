//! Inbound frames the relay pushes down a tagma's tunnel. The tunnel is the
//! tagma's only inbound channel, so it carries forwarded data-plane envelopes
//! and app-initiated key-exchange inits (the control channel that runs *before*
//! a conversation has an E2E key).

use crate::control::KeyExchangeInit;
use crate::message::Envelope;
use kallip_archeion_common::ids::ConversationId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TunnelInbound {
    /// A forwarded E2E envelope for a conversation this tagma owns.
    Envelope { envelope: Envelope },
    /// An app wants to establish a conversation E2E key. The tagma derives the
    /// shared secret and replies with a signed
    /// [`crate::control::KeyExchangeResponse`]. The agent that backs the
    /// conversation is the tagma's own concern and is not carried here.
    KeyExchange {
        conversation_id: ConversationId,
        init: KeyExchangeInit,
    },
    /// A best-effort hint that a room's membership changed, so the tagma should
    /// refresh its joined-rooms cache immediately instead of waiting for the
    /// next poll tick. A Wake is transient and NOT buffered -- an offline tagma
    /// misses it and relies on the room-membership pump's immediate first tick
    /// on reconnect. Fanned to every live tagma of the changed room. Carries no
    /// payload: the tagma re-fetches its full joined-rooms set on receipt.
    Wake,
    /// A manage-plane REST request relayed from the app (via the lesche
    /// reverse-proxy) for the tagma to execute against its manage router.
    /// Unlike [`TunnelInbound::Envelope`], this frame is NOT E2E-encrypted:
    /// manage metadata is deliberately visible to the relay (TLS transport +
    /// device-proof tunnel auth is the trust base; message content stays on
    /// the envelope path). The tagma enforces a frame-surface allowlist
    /// (low/medium-sensitivity routes only; anything unlisted is a 404) so
    /// prompt-bearing routes never traverse this frame in plaintext.
    ManageRest {
        /// Distributed-trace id minted by the lesche reverse proxy per
        /// proxied request -- a fresh UUID, so traces stay collision-free
        /// across tunnel reconnects (req_id restarts at zero on
        /// reconnect; a trace must not).
        req_id: u64,
        method: String,
        path: String,
        trace: kallip_archeion_common::ids::TraceId,
        body: serde_json::Value,
    },
    /// A best-effort hint that the lesche's projection subscription count
    /// crossed the zero <-> non-zero boundary: `true` means at least one
    /// client is reading the projection (the tagma should push projection
    /// updates), `false` means nobody is listening (skip the push, save the
    /// work). Like [`TunnelInbound::Wake`], the hint is transient and NOT
    /// buffered: an offline tagma misses it, so the lesche re-sends the
    /// current state when the tunnel re-establishes, and the tagma treats
    /// tunnel-up as implicitly active (full first snapshot) regardless of
    /// hint history. The tagma-side default is `false` (fail toward saving
    /// resources). Carries no projection data: the tagma recomputes the
    /// snapshot itself on the next push.
    SubscriptionHint { active: bool },
}

/// The plaintext reply to a [`TunnelInbound::ManageRest`] frame: the tagma
/// POSTs this back to the lesche, which resolves the pending proxy request.
/// Plaintext by design -- manage metadata is the relay-visible surface.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManageRestReply {
    pub req_id: u64,
    pub status: u16,
    pub body: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wake_round_trips() {
        let frame = TunnelInbound::Wake;
        let json = serde_json::to_string(&frame).unwrap();
        assert!(json.contains("\"kind\":\"wake\""), "{json}");
        let back: TunnelInbound = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, TunnelInbound::Wake));
    }

    /// The subscription hint round-trips with its snake_case kind tag and
    /// carries the active flag (the fifth inbound frame).
    #[test]
    fn subscription_hint_round_trips() {
        let frame = TunnelInbound::SubscriptionHint { active: true };
        let json = serde_json::to_string(&frame).unwrap();
        assert!(
            json.contains("\"kind\":\"subscription_hint\"") && json.contains("\"active\":true"),
            "{json}"
        );
        let back: TunnelInbound = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            back,
            TunnelInbound::SubscriptionHint { active: true }
        ));
    }
}

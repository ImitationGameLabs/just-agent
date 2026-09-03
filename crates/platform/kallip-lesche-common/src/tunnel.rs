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
        req_id: u64,
        method: String,
        path: String,
        body: serde_json::Value,
    },
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
}

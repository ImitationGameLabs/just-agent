//! The manage-plane projection snapshot the tagma pushes to the lesche
//! (api-redesign §9.1): a full roster plus the aggregate status counters.
//!
//! Plaintext by design -- manage metadata is the relay-visible surface (same
//! trust base as the [`crate::tunnel::TunnelInbound::ManageRest`] frame). The
//! snapshot is a *pure cache* of tagma state: the lesche stores the latest
//! one under a monotonically increasing seq and serves it to clients even
//! when the tagma is offline (stale read), so the tagma simply re-pushes a
//! fresh full snapshot on tunnel-up -- there is no delta protocol.
//!
//! Deliberately excluded (api-redesign §9.1 last bullet): prompt-bearing
//! text never rides the projection. Roster summaries carry only registry
//! metadata (role/description/activity/duty), and the work-schedule
//! projection is deferred until its storage shape lands (P2-b) precisely so
//! its prompt fields (`wake_prompt`, `final_warn_prompt`) cannot leak in.

use crate::event::TagmaStatusPayload;
use kallip_common::protocol::AgentSummary;
use serde::{Deserialize, Serialize};

/// One full-projection push from a tagma. `generated_at` is the tagma-side
/// wall clock (RFC 3339) -- informational; the lesche stamps its own
/// `updated_at` and `seq` on receipt, since those are per-lesche-store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectionSnapshot {
    /// The full agent roster, registry metadata only (no prompt text).
    pub agents: Vec<AgentSummary>,
    /// The aggregate status counters (`snapshot_status` output): root state,
    /// subagent totals, token budget/consumed. Kept as the existing wire
    /// type so the status pump's payload shape is reused verbatim.
    pub status: TagmaStatusPayload,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The snapshot rides the plaintext internal surface, but it is still a
    /// persisted-then-served wire shape: pin the field names so a rename
    /// cannot silently break the lesche's stored rows across a deploy.
    #[test]
    fn snapshot_field_names_are_wire_stable() {
        let json = serde_json::json!({
            "agents": [],
            "status": {
                "root_state": "idle",
                "subagents_total": 0,
                "subagents_active": 0,
                "token_budget": 1,
                "token_consumed": 0,
            },
        });
        let snap: ProjectionSnapshot = serde_json::from_value(json).expect("parses");
        assert!(snap.agents.is_empty());
        assert_eq!(
            snap.status.root_state,
            kallip_common::protocol::AgentState::Idle
        );
        assert_eq!(snap.status.subagents_total, 0);
    }
}

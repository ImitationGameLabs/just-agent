//! The relay's shared state: a handle to the registry ([`ControlPlane`]) plus
//! the in-memory soft-state `Registry` (presence, conversations, app streams)
//! and the relay-only `pending_key_exchange` window.
//!
//! Everything here is soft-state, rebuilt on restart (presence from tagmata
//! reconnecting, conversations create-on-demand). The durable identity /
//! credential / provisioning layer lives in the registry behind
//! [`ControlPlane`]; this crate never reads or writes it directly. The relay
//! keeps NO replay/dedup window: `sequence_n` is an end-to-end (app<->
//! tagma) counter scoped to a crypto epoch the relay cannot see, so replay
//! protection lives entirely at the tagma (per-epoch `seen_inbound` + AEAD
//! key rotation).
//!
//! # Lock-discipline invariants (authoritative)
//!
//! 1. **No `.await` under a lock.** Drop every `read()`/`write()`/
//!    `pending_key_exchange` guard before awaiting. `ControlPlane` calls (which
//!    await) happen outside any relay lock.
//! 2. **Never co-hold the registry lock with `pending_key_exchange`.** Register
//!    a KEX waiter only after the registry guard is dropped.
//! 3. **`app_streams` has a single creator: `me_events`** (`routes/events.rs`).
//!    Inserting elsewhere would violate the `OnDrop` cleanup assumption (every
//!    entry has a live subscriber).
//! 4. **`pending_key_exchange` cleanup is unconditional** via `KexGuard`
//!    (`routes/conversations.rs`).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use kallip_archeion_common::control_plane::ControlPlane;
use kallip_archeion_common::ids::{ConversationId, ParticipantId, TagmaId, UserId};
use kallip_common::protocol::ApiError;
use kallip_lesche_common::control::KeyExchangeResponse;
use kallip_lesche_common::event::{LescheEvent, TagmaStatusPayload};
use kallip_lesche_common::projection::{ProjectionDirty, ProjectionSnapshot};
use kallip_lesche_common::rooms::MemberId;
use kallip_lesche_common::tunnel::TunnelInbound;
use tokio::sync::{broadcast, oneshot};

pub type SharedConvState = Arc<ConversationsState>;

/// Capacity of the per-tagma / per-user broadcast channel feeding an SSE stream.
pub const BROADCAST_CAPACITY: usize = 128;

/// The relay state. The registry is reached only through `control`; the rest is
/// in-memory, per-incarnation.
pub struct ConversationsState {
    /// The registry (identity + tagma metadata + replay guard), DB-backed in
    /// production by `kallip-archeion`, mockable in tests.
    pub control: Arc<dyn ControlPlane>,
    pub registry: RwLock<Registry>,
    /// Outstanding synchronous key exchanges, keyed by conversation. Bounded by
    /// in-flight KEX. Never held together with the registry lock.
    pub pending_key_exchange:
        std::sync::Mutex<HashMap<ConversationId, oneshot::Sender<KeyExchangeResponse>>>,
    /// Acceptable clock skew (both directions) on a tunnel reconnect proof's
    /// timestamp, in seconds.
    pub proof_skew_secs: i64,
    /// How long `key_exchange_init` waits for the tagma's response before 504.
    pub key_exchange_timeout: Duration,
    /// The durable chat store (`Some` in production): room messages + the
    /// membership graph. Required for room delivery and management.
    pub db: Option<crate::db::Db>,
    /// Authoritative agent identity (label + owner display), keyed by the
    /// agent's `ParticipantId`, populated at tunnel-establish and stamped onto
    /// room envelopes so a tagma cannot self-declare its handle. See
    /// [`AgentProfileCache`].
    pub agent_profiles: AgentProfileCache,
}

/// An agent's authoritative display identity, resolved from the registry and
/// stamped onto room envelopes/rows by the relay. Mirrors the display fields of
/// [`kallip_archeion_common::control_plane::TagmaProfile`] minus the raw usability
/// facts + pinned key (which only the tunnel-proof / policy paths need).
#[derive(Debug, Clone)]
pub struct AgentProfile {
    /// Read by the roster path (a follow-up); the message-stamp path builds the
    /// stable handle from `owner_username` only.
    #[allow(dead_code)]
    pub label: Option<String>,
    pub owner_username: String,
    /// Read by the roster/history path (a follow-up); the message-stamp path
    /// uses only `owner_username`.
    #[allow(dead_code)]
    pub owner_display_name: Option<String>,
}

/// Per-incarnation cache of resolved agent profiles, keyed by the agent's
/// `ParticipantId`. Populated at tunnel-establish (one registry RPC per
/// connect); the rooms send path reads it to stamp the sender handle without a
/// per-message RPC. A cache miss at send falls back to a live `tagma_profile`
/// call and caches the result. Stale on owner rename until tunnel reconnect
/// (acceptable: ownership is non-transferable and the unforgeable id-prefix
/// still disambiguates).
#[derive(Debug, Default)]
pub struct AgentProfileCache(std::sync::Mutex<HashMap<ParticipantId, AgentProfile>>);

impl AgentProfileCache {
    pub fn get(&self, pid: &ParticipantId) -> Option<AgentProfile> {
        self.0.lock().ok()?.get(pid).cloned()
    }
    pub fn set(&self, pid: ParticipantId, profile: AgentProfile) {
        if let Ok(mut g) = self.0.lock() {
            g.insert(pid, profile);
        }
    }
}

impl ConversationsState {
    /// Read-lock the registry, mapping poisoning into an HTTP 500.
    pub fn read(&self) -> Result<std::sync::RwLockReadGuard<'_, Registry>, ApiError> {
        self.registry
            .read()
            .map_err(|e| ApiError::internal(format_args!("registry lock poisoned: {e}")))
    }

    /// The durable store, or a 500. Room management mutates
    /// the durable chat graph, so it cannot degrade to in-memory mock mode
    /// the way delivery does (which skips persistence when no store is wired).
    pub fn require_db(&self) -> Result<&crate::db::Db, ApiError> {
        self.db
            .as_ref()
            .ok_or_else(|| ApiError::internal(format_args!("chat store required")))
    }

    /// Write-lock the registry, mapping poisoning into an HTTP 500.
    pub fn write(&self) -> Result<std::sync::RwLockWriteGuard<'_, Registry>, ApiError> {
        self.registry
            .write()
            .map_err(|e| ApiError::internal(format_args!("registry lock poisoned: {e}")))
    }
}

/// In-memory index of presence, conversations, and per-user app streams.
///
/// The presence / app-stream maps are keyed by
/// [`ParticipantId`] (the opaque room-layer identity): a tagma's tunnel and a
/// user's app stream are both looked up by their derived participant id, so the
/// room fan-out routes every member uniformly regardless of kind. The
/// `TagmaId`/`UserId`-taking helpers derive the participant id internally, so
/// the tagma-lifecycle paths (tunnel/status/signal) and the app-stream paths
/// (`me_events`) keep their existing call shapes; the room fan-out uses the
/// `_by_member` lookups.
pub struct Registry {
    pub conversations: HashMap<ConversationId, ConversationRecord>,
    /// participant_id -> the member's live tunnel (Agent members only today -- a
    /// platform-native tagma). A participant is "online" iff it has an entry.
    /// `owner` routes presence events to the owning user; `id` is a
    /// per-connection identity token so a stale tunnel's cleanup cannot remove a
    /// freshly reconnected tunnel's presence.
    pub presence: HashMap<ParticipantId, PresenceEntry>,
    /// participant_id -> outbound broadcast to that member's multiplexed app SSE
    /// (Human members). The sole creator is `me_events`; it carries agent
    /// envelopes and presence events. Private: mutate only via
    /// [`Registry::open_app_stream`] / [`Registry::remove_app_stream_if_last`].
    app_streams: HashMap<ParticipantId, broadcast::Sender<LescheEvent>>,
    /// Per-tagma projection cache (api-redesign §9.5: registry bypass) and
    /// per-(user, tagma) projection SSE channels. Private: mutated only via
    /// the projection methods below. The projection table outlives the
    /// tagma's presence (MIN3): offline tags keep serving stale reads.
    projections: HashMap<ParticipantId, ProjectionEntry>,
    projection_streams: HashMap<(ParticipantId, ParticipantId), broadcast::Sender<ProjectionDirty>>,
    /// Teardown lag for projection SSE unsubscribes, in milliseconds (30s
    /// in production; tests shorten it). Read by the SSE OnDrop cleanup.
    projection_unsub_lag_ms: std::sync::atomic::AtomicU64,
}

/// One live tunnel: the outbound broadcast, the owning user (for presence
/// routing), the tagma id (carried for the tagma-lifecycle event layer, which
/// still speaks `TagmaId` -- the participant-id key is one-way derived and
/// cannot be reversed for `TagmaOnline`/`TagmaStatus` payloads), and a
/// per-connection identity token used to make presence removal race-free across
/// reconnects.
pub struct PresenceEntry {
    pub tx: broadcast::Sender<TunnelInbound>,
    pub owner: UserId,
    pub tagma_id: TagmaId,
    pub id: Arc<()>,
    /// Latest aggregate status snapshot relayed over this tunnel; written unconditionally on every status POST (with or without live subscribers). Tunnel-scoped: a reconnect starts the cache empty until the next pump tick (bounded self-heal, <= one heartbeat period).
    pub latest_status: Option<TagmaStatusPayload>,
}
/// One tagma's stored projection: the latest accepted push plus the
/// bookkeeping the accept/replay logic needs. `generation` records which
/// tunnel connection the snapshot came in on (an `Arc::ptr_eq` against the
/// live [`PresenceEntry::id`]): a push from a different generation is a
/// reconnect whose push counter restarted, so it is accepted
/// unconditionally (M1); a same-generation push must carry a strictly
/// newer `push_seq` or it is an out-of-order replay.
#[derive(Clone)]
pub struct ProjectionEntry {
    /// Store-side seq: increments once per accepted push.
    pub seq: u64,
    /// The owning user, captured at accept time from the tagma's presence:
    /// offline tags keep serving stale reads (MIN3), and this is what makes
    /// the C1 owner check possible without a live presence entry.
    pub owner: UserId,
    /// The tagma-side push counter at accept time.
    pub last_push_seq: u64,
    /// Wall-clock unix seconds when the push was accepted.
    pub updated_at: i64,
    pub generation: Arc<()>,
    pub snapshot: ProjectionSnapshot,
}

#[derive(Debug, Clone)]
pub struct ConversationRecord {
    pub owner: UserId,
    pub tagma_id: TagmaId,
}

impl Registry {
    pub fn new() -> Self {
        Self {
            conversations: HashMap::new(),
            presence: HashMap::new(),
            app_streams: HashMap::new(),
            projections: HashMap::new(),
            projection_streams: HashMap::new(),
            projection_unsub_lag_ms: std::sync::atomic::AtomicU64::new(30_000),
        }
    }

    /// The live app event-stream sender for `user`, if any. Read-only access for
    /// routing agent envelopes and presence events; creation is
    /// [`Self::open_app_stream`]. Keyed by the user's derived participant id.
    pub fn app_stream(&self, user: &UserId) -> Option<&broadcast::Sender<LescheEvent>> {
        self.app_streams.get(&ParticipantId::for_user(user))
    }

    /// The live app-stream sender for a room member id (the room fan-out's
    /// Human-member lookup). The map keys on the underlying `ParticipantId`; a
    /// `MemberId` shares that UUID, so the lookup goes by the borrowed string.
    /// Takes the room-domain `MemberId` (not the raw `UserId`) so a caller cannot
    /// accidentally look up by an account id whose string is not the derived key.
    pub fn app_stream_by_member(&self, mid: &MemberId) -> Option<&broadcast::Sender<LescheEvent>> {
        self.app_streams.get(mid.as_ref())
    }

    /// Ensure an app event-stream channel exists for `user` and return a sender
    /// clone. Sole creator of `app_streams` entries.
    pub fn open_app_stream(&mut self, user: &UserId) -> broadcast::Sender<LescheEvent> {
        let pid = ParticipantId::for_user(user);
        self.app_streams
            .entry(pid)
            .or_insert_with(|| broadcast::channel::<LescheEvent>(BROADCAST_CAPACITY).0)
            .clone()
    }

    /// Remove `user`'s app-stream channel iff `sender` is the last subscriber
    /// (`receiver_count() == 1`: the dying SSE stream itself). Returns whether the
    /// entry was removed (the user has no remaining app stream) so the caller can
    /// fan a presence-offline transition only on the 1 -> 0 edge.
    pub fn remove_app_stream_if_last(
        &mut self,
        user: &UserId,
        sender: &broadcast::Sender<LescheEvent>,
    ) -> bool {
        if sender.receiver_count() == 1 {
            self.app_streams.remove(&ParticipantId::for_user(user));
            true
        } else {
            false
        }
    }

    /// Whether `user` currently holds an app stream. Named for intent at the
    /// 0 -> 1 edge detection in `me_events` (announce presence only on the first
    /// stream); semantically equivalent to `self.app_stream(user).is_some()`.
    /// Only meaningful under the registry write lock the caller already holds.
    pub fn has_app_stream(&self, user: &UserId) -> bool {
        self.app_streams
            .contains_key(&ParticipantId::for_user(user))
    }

    // --- Projection store & streams (api-redesign §9.1/§9.5/§9.7) ---

    /// Accept a projection push from `tagma` (whose live presence
    /// connection token is `generation`). Returns the new store seq, or
    /// `None` when the push is a same-generation replay (its `push_seq` is
    /// not newer than the accepted one) -- such a push is dropped without
    /// touching the store or bumping the seq.
    pub fn accept_projection(
        &mut self,
        tagma: &TagmaId,
        generation: &Arc<()>,
        push_seq: u64,
        owner: UserId,
        snapshot: ProjectionSnapshot,
    ) -> Option<u64> {
        let pid = ParticipantId::for_tagma(tagma);
        let next_seq = match self.projections.get(&pid) {
            Some(stored) => {
                let same = Arc::ptr_eq(&stored.generation, generation);
                if same && push_seq <= stored.last_push_seq {
                    return None; // out-of-order replay within a generation
                }
                // Store seq keeps climbing across generations: clients use
                // it to detect a reset, so only the tagma push_seq
                // expectation is voided by a new generation (M1).
                stored.seq + 1
            }
            None => 1,
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.projections.insert(
            pid,
            ProjectionEntry {
                owner,
                seq: next_seq,
                last_push_seq: push_seq,
                updated_at: now,
                generation: generation.clone(),
                snapshot,
            },
        );
        Some(next_seq)
    }

    /// The stored projection for `tagma`, if any push was ever accepted.
    /// Outlives the tagma's presence (MIN3): offline tags keep serving
    /// stale reads from this.
    pub fn projection(&self, tagma: &TagmaId) -> Option<&ProjectionEntry> {
        self.projections.get(&ParticipantId::for_tagma(tagma))
    }

    /// Open (or join) the (user, tagma) projection SSE channel. Returns the
    /// receiver plus whether this subscribe is the stream's 0 -> 1 edge (the
    /// caller fans a `SubscriptionHint { active: true }` on that edge only).
    pub fn open_projection_stream(
        &mut self,
        user: &UserId,
        tagma: &TagmaId,
    ) -> (
        broadcast::Sender<ProjectionDirty>,
        broadcast::Receiver<ProjectionDirty>,
        bool,
    ) {
        let key = (
            ParticipantId::for_user(user),
            ParticipantId::for_tagma(tagma),
        );
        let sender = self
            .projection_streams
            .entry(key)
            .or_insert_with(|| broadcast::channel::<ProjectionDirty>(BROADCAST_CAPACITY).0)
            .clone();
        let was_first = sender.receiver_count() == 0;
        let rx = sender.subscribe();
        if was_first {
            self.send_subscription_hint(tagma, true);
        }
        (sender, rx, was_first)
    }

    /// Fan `SubscriptionHint { active }` down the tagma's tunnel. Best-effort:
    /// an offline tagma misses it; the tunnel handler re-sends the current
    /// state on every reconnect so the flag can never go stale for long.
    pub fn send_subscription_hint(&self, tagma: &TagmaId, active: bool) {
        if let Some(entry) = self.presence.get(&ParticipantId::for_tagma(tagma)) {
            let _ = entry.tx.send(TunnelInbound::SubscriptionHint { active });
        }
    }

    /// Remove the (user, tagma) projection channel when `sender` is its last
    /// subscriber, mirroring [`Self::remove_app_stream_if_last`]. Returns
    /// whether the entry was removed (the 1 -> 0 edge; the caller fans
    /// `SubscriptionHint { active: false }` after the lag window).
    pub fn remove_projection_stream_if_last(
        &mut self,
        user: &UserId,
        tagma: &TagmaId,
        sender: &broadcast::Sender<ProjectionDirty>,
    ) -> bool {
        let key = (
            ParticipantId::for_user(user),
            ParticipantId::for_tagma(tagma),
        );
        // By the time the OnDrop cleanup runs, the dying stream's own rx is
        // already gone, so `receiver_count() == 0` means "nobody left"; a
        // new subscriber that arrived inside the lag window keeps the
        // channel (and the hint) alive.
        if sender.receiver_count() == 0 {
            self.projection_streams.remove(&key);
            self.send_subscription_hint(tagma, false);
            true
        } else {
            false
        }
    }
    /// The projection SSE teardown lag (ms). Production is 30s; tests
    /// shorten it so the hint-flap window is observable.
    pub fn projection_unsub_lag_ms(&self) -> u64 {
        self.projection_unsub_lag_ms
            .load(std::sync::atomic::Ordering::Relaxed)
    }
    #[cfg(test)] // consumed by the SSE OnDrop teardown test only
    /// Shorten the teardown lag (test injection only).
    pub fn set_projection_unsub_lag_ms(&self, ms: u64) {
        self.projection_unsub_lag_ms
            .store(ms, std::sync::atomic::Ordering::Relaxed);
    }

    /// Whether any client currently holds a live projection stream for
    /// `tagma`. Consulted on tunnel reconnect so the lesche re-sends the
    /// current hint (the tagma may have missed a flip while offline).
    pub fn projection_stream_live_for_tagma(&self, tagma: &TagmaId) -> bool {
        let tp = ParticipantId::for_tagma(tagma);
        self.projection_streams
            .iter()
            .any(|((_, t), s)| *t == tp && s.receiver_count() > 0)
    }

    /// Fan a `ProjectionDirty` frame to `(user, tagma)`'s projection stream,
    /// if any client is subscribed. Best-effort: no subscribers is fine (the
    /// hint suppression means this is the normal unread case).
    pub fn fan_projection_dirty(&self, user: &UserId, tagma: &TagmaId, dirty: ProjectionDirty) {
        if let Some(sender) = self.projection_streams.get(&(
            ParticipantId::for_user(user),
            ParticipantId::for_tagma(tagma),
        )) {
            let _ = sender.send(dirty);
        }
    }

    /// Ensure the soft-state conversation record exists for `tagma_id` owned by
    /// `owner`, and return its stable id. Idempotent (the id is the
    /// deterministic `ConversationId::for_tagma` derivation). Sole mutator of
    /// `conversations`.
    pub fn ensure_conversation(&mut self, owner: &UserId, tagma_id: &TagmaId) -> ConversationId {
        let conv_id = ConversationId::for_tagma(tagma_id);
        self.conversations
            .entry(conv_id.clone())
            .or_insert(ConversationRecord {
                owner: owner.clone(),
                tagma_id: tagma_id.clone(),
            });
        conv_id
    }

    /// Register a live tagma tunnel for `tagma`, owned by `owner`, capturing the
    /// per-connection identity token `id` so a stale tunnel's cleanup cannot
    /// remove a fresh reconnect's presence. Keyed by the tagma's derived
    /// participant id.
    pub fn register_presence(
        &mut self,
        tagma: &TagmaId,
        owner: UserId,
        tx: broadcast::Sender<TunnelInbound>,
        id: Arc<()>,
    ) {
        self.presence.insert(
            ParticipantId::for_tagma(tagma),
            PresenceEntry {
                tx,
                owner,
                tagma_id: tagma.clone(),
                id,
                latest_status: None,
            },
        );
    }

    /// The live tunnel entry for a room member id (the room fan-out's Agent-member
    /// lookup). See [`app_stream_by_member`](Self::app_stream_by_member) for the
    /// keying rationale and the `MemberId`-typed guard.
    pub fn presence_by_member(&self, mid: &MemberId) -> Option<&PresenceEntry> {
        self.presence.get(mid.as_ref())
    }

    /// The live tunnel entry for a tagma (the tagma-lifecycle paths: tunnel /
    /// status / signal). Derives the participant id internally so those paths
    /// keep their `TagmaId` call shape.
    pub fn presence_by_tagma(&self, tagma: &TagmaId) -> Option<&PresenceEntry> {
        self.presence.get(&ParticipantId::for_tagma(tagma))
    }

    /// Mutable lookup for the status relay's cache write (the only writer of latest_status). See presence_by_tagma.
    pub fn presence_by_tagma_mut(&mut self, tagma: &TagmaId) -> Option<&mut PresenceEntry> {
        self.presence.get_mut(&ParticipantId::for_tagma(tagma))
    }
    /// Remove `tagma`'s presence iff the live entry is still `id` (Arc pointer
    /// identity), returning whether it was removed. Race-free across reconnects.
    pub fn take_presence_if_owned(&mut self, tagma: &TagmaId, id: &Arc<()>) -> bool {
        let pid = ParticipantId::for_tagma(tagma);
        let still_ours = self
            .presence
            .get(&pid)
            .map(|p| Arc::ptr_eq(&p.id, id))
            .unwrap_or(false);
        if still_ours {
            self.presence.remove(&pid);
        }
        still_ours
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kallip_archeion_common::ids::TagmaId;
    use kallip_lesche_common::event::LescheEvent;

    /// `register_presence` stores the owner so presence events can be routed to
    /// the owning user's app stream, and the snapshot iteration (what
    /// `me_events` emits on stream open) filters by owner.
    #[test]
    fn presence_records_owner_and_snapshot_filters_by_owner() {
        let mut reg = Registry::new();
        let alice = UserId::from("alice".to_string());
        let bob = UserId::from("bob".to_string());
        let a1 = TagmaId::from("a1".to_string());
        let a2 = TagmaId::from("a2".to_string());
        let b1 = TagmaId::from("b1".to_string());
        let (tx_a1, _) = broadcast::channel::<TunnelInbound>(8);
        let (tx_a2, _) = broadcast::channel::<TunnelInbound>(8);
        let (tx_b1, _) = broadcast::channel::<TunnelInbound>(8);
        reg.register_presence(&a1, alice.clone(), tx_a1, Arc::new(()));
        reg.register_presence(&a2, alice.clone(), tx_a2, Arc::new(()));
        reg.register_presence(&b1, bob.clone(), tx_b1, Arc::new(()));

        // Snapshot for alice = her two tagmas.
        let alice_online: Vec<TagmaId> = reg
            .presence
            .values()
            .filter(|e| e.owner == alice)
            .map(|e| e.tagma_id.clone())
            .collect();
        assert_eq!(alice_online.len(), 2);
        assert!(alice_online.contains(&a1) && alice_online.contains(&a2));
    }

    /// The presence-push wiring: an open app stream for the owner receives a
    /// `TagmaOnline` sent to the owner's sender (the path `tunnel`/`me_events`
    /// take on connect / stream open).
    #[tokio::test]
    async fn app_stream_receives_presence_event() {
        let mut reg = Registry::new();
        let alice = UserId::from("alice".to_string());
        let tx = reg.open_app_stream(&alice);
        let mut rx = tx.subscribe();
        // Simulate the tunnel handler's online announcement.
        tx.send(LescheEvent::TagmaOnline {
            tagma_id: TagmaId::from("a1".to_string()),
        })
        .expect("send");
        let ev = rx.recv().await.expect("receive");
        assert!(matches!(ev, LescheEvent::TagmaOnline { .. }));
    }
}

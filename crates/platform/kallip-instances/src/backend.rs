//! The instance backend: the transport seam behind the four management
//! routes. The local implementation is a thin wrapper over the daemon's
//! UDS client; any other source (an orchestration API) implements the
//! same trait without the HTTP layer knowing the difference.
//!
//! The trait stays in this crate (not a `-common` package) until a second
//! consumer exists -- same rule as the `BearerVerifier` seam.

use std::sync::Arc;

use async_trait::async_trait;
use kallip_daemon_client::DaemonClient;
use kallip_daemon_common::wire::{HealthReport, InstanceInfo, RequestBody};

use crate::api::{Spawned, Stopped};
use crate::wire::{BackendError, unwrap_health, unwrap_list, unwrap_spawn, unwrap_stop};

/// The full management surface, one method per HTTP route.
#[async_trait]
pub trait InstanceBackend: Send + Sync + 'static {
    /// Launch one instance; the outcome carries its listen port.
    async fn spawn(
        &self,
        slug: String,
        workspace: String,
        env: Vec<String>,
    ) -> Result<Spawned, BackendError>;

    /// Stop one instance by slug.
    async fn stop(&self, slug: String) -> Result<Stopped, BackendError>;

    /// Relaunch a stopped or dead instance from its persisted tree.
    /// The outcome is spawn-shaped: a fresh process with its listen port.
    async fn start(&self, slug: String) -> Result<Spawned, BackendError>;

    /// Every instance the backend sees.
    async fn list(&self) -> Result<Vec<InstanceInfo>, BackendError>;

    /// Liveness of the backend itself, or one instance by slug.
    async fn health(&self, slug: Option<String>) -> Result<HealthReport, BackendError>;
    /// Provisioning methods this backend supports, as the shared
    /// product vocabulary (designated-user / isolated-user /
    /// container). The web renders the create flow from this list;
    /// an empty set hides the create entry, not the page.
    fn capabilities(&self) -> Vec<String>;
}

/// The local backend: one UDS exchange per call against the host daemon.
pub struct UdsBackend {
    /// Server-side relay-URL defaults injected on spawn (see
    /// `fill_relay_defaults`); the local-backend scope keeps them plain
    /// strings.
    relay_archeion_url: String,
    relay_lesche_url: String,
    client: DaemonClient,
}

impl UdsBackend {
    pub fn arc(client: DaemonClient) -> Arc<dyn InstanceBackend> {
        Self::arc_with_relays(client, String::new(), String::new())
    }

    /// `arc` with the server-side relay-URL defaults wired in from Config.
    pub fn arc_with_relays(
        client: DaemonClient,
        relay_archeion_url: String,
        relay_lesche_url: String,
    ) -> Arc<dyn InstanceBackend> {
        Arc::new(Self {
            relay_archeion_url,
            relay_lesche_url,
            client,
        })
    }
}

#[async_trait]
impl InstanceBackend for UdsBackend {
    async fn spawn(
        &self,
        slug: String,
        workspace: String,
        env: Vec<String>,
    ) -> Result<Spawned, BackendError> {
        // Relay-intent default injection (local backend only): a spawn
        // env carrying any KALLIP_TAGMA_RELAY_* entry gets the missing
        // URLs filled from the server-side defaults; explicit values
        // pass through untouched, and no RELAY_* at all means local-
        // only -- nothing is injected (the tagma boot fails fast on a
        // URL without enrollment).
        let env = fill_relay_defaults(env, &self.relay_archeion_url, &self.relay_lesche_url);
        let wire = self
            .client
            .call(RequestBody::Spawn {
                slug,
                workspace,
                env,
                exe: None,
            })
            .await?;
        unwrap_spawn(wire)
    }

    async fn stop(&self, slug: String) -> Result<Stopped, BackendError> {
        let wire = self.client.call(RequestBody::Stop { slug }).await?;
        unwrap_stop(wire)
    }

    async fn start(&self, slug: String) -> Result<Spawned, BackendError> {
        // The daemon answers start with the shared spawn-shaped launch
        // payload, so the unwrap is verbatim.
        let wire = self
            .client
            .call(RequestBody::Start {
                slug,
                env: Vec::new(),
                exe: None,
            })
            .await?;
        unwrap_spawn(wire)
    }

    async fn list(&self) -> Result<Vec<InstanceInfo>, BackendError> {
        let wire = self.client.call(RequestBody::List).await?;
        unwrap_list(wire)
    }

    async fn health(&self, slug: Option<String>) -> Result<HealthReport, BackendError> {
        let wire = self.client.call(RequestBody::Health { slug }).await?;
        unwrap_health(wire)
    }
    fn capabilities(&self) -> Vec<String> {
        vec!["designated-user".to_string()]
    }
}

// Re-exported so handler code names one error type regardless of backend.
pub use crate::wire::BackendError as Error;

/// Fill the relay-URL entries with the server-side defaults.
///
/// Fires only when the env signals relay intent (any
/// `KALLIP_TAGMA_RELAY_*` entry): each URL key survives exactly
/// once, with its explicit value or -- when empty or absent -- the
/// default. No relay signal at all returns the env unchanged:
/// local-only spawns must not carry a URL the tagma boot would
/// fail on.
fn fill_relay_defaults(env: Vec<String>, archeion: &str, lesche: &str) -> Vec<String> {
    let mut env = env;
    if !env.iter().any(|e| e.starts_with("KALLIP_TAGMA_RELAY_")) {
        return env;
    }
    fill_one(&mut env, "KALLIP_TAGMA_RELAY_ARCHEION_URL=", archeion);
    fill_one(&mut env, "KALLIP_TAGMA_RELAY_LESCHE_URL=", lesche);
    env
}

/// One entry per key survives: the first explicit value wins
/// outright, an empty or absent key is filled with the default,
/// and every duplicate is dropped. Ordering must stay irrelevant
/// downstream -- the spawn helper passes duplicate pairs straight
/// to execve, and the daemon's env validation rejects empty
/// values -- so collapsing here is the one place that keeps both
/// properties airtight.
fn fill_one(env: &mut Vec<String>, prefix: &str, default: &str) {
    let explicit = env
        .iter()
        .any(|e| e.strip_prefix(prefix).is_some_and(|v| !v.is_empty()));
    let mut kept = false;
    env.retain(|e| {
        let Some(v) = e.strip_prefix(prefix) else {
            return true;
        };
        if kept || (explicit && v.is_empty()) {
            return false;
        }
        kept = true;
        true
    });
    if explicit {
        return;
    }
    match env.iter_mut().find(|e| e.starts_with(prefix)) {
        Some(slot) => *slot = format!("{prefix}{default}"),
        None => env.push(format!("{prefix}{default}")),
    }
}

#[cfg(test)]
mod tests {
    use super::fill_relay_defaults;

    const ARCHEION: &str = "http://localhost:7100";
    const LESCHE: &str = "http://localhost:7200";

    fn has(env: &[String], prefix: &str) -> bool {
        env.iter().any(|e| e.starts_with(prefix))
    }

    /// State A: relay intent without URLs -- both defaults are filled in.
    #[test]
    fn relay_intent_code_only_fills_both_urls() {
        let out = fill_relay_defaults(
            vec!["KALLIP_TAGMA_RELAY_ENROLLMENT_CODE=sk-x".into()],
            ARCHEION,
            LESCHE,
        );
        assert!(has(
            &out,
            "KALLIP_TAGMA_RELAY_ARCHEION_URL=http://localhost:7100"
        ));
        assert!(has(
            &out,
            "KALLIP_TAGMA_RELAY_LESCHE_URL=http://localhost:7200"
        ));
    }

    /// State B: no relay signal -- nothing is injected.
    #[test]
    fn local_only_env_stays_untouched() {
        let out = fill_relay_defaults(
            vec!["KALLIP_LLM_PROVIDER=deepseek".into()],
            ARCHEION,
            LESCHE,
        );
        assert!(!has(&out, "KALLIP_TAGMA_RELAY_"));
        assert_eq!(out.len(), 1);
    }

    /// Explicit values win; empty entries count as missing and are
    /// replaced in place (no duplicate keys).
    #[test]
    fn explicit_values_win_and_empty_is_filled_in_place() {
        let out = fill_relay_defaults(
            vec![
                "KALLIP_TAGMA_RELAY_ARCHEION_URL=https://archeion.example.com".into(),
                "KALLIP_TAGMA_RELAY_LESCHE_URL=".into(),
            ],
            ARCHEION,
            LESCHE,
        );
        assert!(has(
            &out,
            "KALLIP_TAGMA_RELAY_ARCHEION_URL=https://archeion.example.com"
        ));
        assert!(has(
            &out,
            "KALLIP_TAGMA_RELAY_LESCHE_URL=http://localhost:7200"
        ));
        assert_eq!(out.len(), 2);
    }

    /// Duplicate entries collapse to the single explicit value
    /// regardless of order: an empty duplicate must neither gain
    /// the default (an order-sensitive consumer could let it
    /// win) nor survive (the daemon's env validation rejects
    /// empty values).
    #[test]
    fn duplicate_keys_collapse_to_the_explicit_value() {
        let out = fill_relay_defaults(
            vec![
                "KALLIP_TAGMA_RELAY_ARCHEION_URL=".into(),
                "KALLIP_TAGMA_RELAY_ENROLLMENT_CODE=sk-x".into(),
                "KALLIP_TAGMA_RELAY_ARCHEION_URL=https://archeion.example.com".into(),
            ],
            ARCHEION,
            LESCHE,
        );
        assert!(has(
            &out,
            "KALLIP_TAGMA_RELAY_ARCHEION_URL=https://archeion.example.com"
        ));
        assert_eq!(
            out.iter()
                .filter(|e| e.starts_with("KALLIP_TAGMA_RELAY_ARCHEION_URL"))
                .count(),
            1
        );
        // Reversed order: an empty duplicate ahead of the explicit one.
        let out = fill_relay_defaults(
            vec![
                "KALLIP_TAGMA_RELAY_LESCHE_URL=".into(),
                "KALLIP_TAGMA_RELAY_LESCHE_URL=https://lesche.example.com".into(),
            ],
            ARCHEION,
            LESCHE,
        );
        assert!(has(
            &out,
            "KALLIP_TAGMA_RELAY_LESCHE_URL=https://lesche.example.com"
        ));
        assert_eq!(
            out.iter()
                .filter(|e| e.starts_with("KALLIP_TAGMA_RELAY_LESCHE_URL"))
                .count(),
            1
        );
        // Two empties fill once, never duplicate.
        let out = fill_relay_defaults(
            vec![
                "KALLIP_TAGMA_RELAY_ARCHEION_URL=".into(),
                "KALLIP_TAGMA_RELAY_ARCHEION_URL=".into(),
            ],
            ARCHEION,
            LESCHE,
        );
        assert_eq!(out.len(), 2, "archeion collapsed plus the lesche default");
        assert!(has(
            &out,
            "KALLIP_TAGMA_RELAY_ARCHEION_URL=http://localhost:7100"
        ));
    }
}

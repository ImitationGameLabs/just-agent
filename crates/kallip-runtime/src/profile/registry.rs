//! The profile registry: named profile sets over named providers, backed by a
//! [`BackendSource`].
//!
//! Sets are addressed by name (exact match, case-sensitive); the chosen name is
//! persisted on the agent record at spawn. The active profile is `set.profiles[0]`;
//! the rest of the chain is the within-set failover order, whose backends are built
//! lazily on first use. Cross-set failover is intentionally off.

use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use just_llm_client::{ChatClient, ChatClientOptions, LlmBackend};

use super::model::{Profile, ProfileSet};

/// Lazily provides [`LlmBackend`]s keyed by provider id. The tagma owns the implementation
/// (reqwest + [`just_llm_client::client::BackendFactory`]); the registry looks up the active
/// profile's backend via [`get`](Self::get), and a failover profile's backend is built on first
/// use. Implementations must construct a given provider's backend at most once under concurrent
/// access (e.g. a locked cache), so repeated lookups share one backend.
pub trait BackendSource: Send + Sync {
    fn get(&self, provider_id: &str) -> Result<Arc<dyn LlmBackend>>;
}

pub struct ProfileRegistry {
    sets: BTreeMap<String, ProfileSet>,
    /// Backends keyed by provider id. The active set is pre-built at tagma startup; failover
    /// endpoints are built lazily by the [`BackendSource`] on first lookup. The registry itself
    /// never constructs — it calls [`BackendSource::get`].
    source: Arc<dyn BackendSource>,
}

impl ProfileRegistry {
    /// Construct and validate: every set non-empty, and each set's `name` synchronized with
    /// its map key (deserialization cannot fill it — `name` is skipped on
    /// the wire — so this constructor is the single normalization point,
    /// unconditionally overwriting `name` with the key; the name can never
    /// disagree with the key it is addressed by). An empty collection is
    /// allowed (profile-less boot — select errors per call until a profile is
    /// added). Provider existence, family, and base_url are validated by the
    /// tagma when it builds the active set (see `kallip_tagma::backend`);
    /// the registry only checks structure.
    pub fn new(
        mut sets: BTreeMap<String, ProfileSet>,
        source: Arc<dyn BackendSource>,
    ) -> Result<Self> {
        for (key, set) in sets.iter_mut() {
            if set.profiles.is_empty() {
                bail!("set '{key}' has no profiles");
            }
            set.name = key.clone();
        }
        Ok(Self { sets, source })
    }

    pub fn sets(&self) -> &BTreeMap<String, ProfileSet> {
        &self.sets
    }

    /// Resolve a set by its exact name. Unknown names error with the available set names
    /// listed, so the caller surfaces an actionable message (spawn rejects with it; runtime
    /// rebinds treat it as a dangling record).
    pub fn select_set(&self, name: &str) -> Result<&ProfileSet> {
        self.sets.get(name).ok_or_else(|| {
            let known: Vec<&str> = self.sets.keys().map(String::as_str).collect();
            anyhow::anyhow!(
                "unknown profile set '{name}'; available sets: {}",
                known.join(", ")
            )
        })
    }

    /// Resolve the agent's set by supervisor depth, positional over the sorted set names:
    /// `sets.values()[depth.min(len-1)]`. Errors with a management-page hint when the
    /// registry is empty (profile-less boot).
    ///
    /// Returns the resolved [`ProfileSet`] handle so the caller (and the failover loop) can
    /// walk `set.profiles`. The active profile is always `set.profiles[0]`. Callers that need
    /// to know whether `depth` was clamped (e.g. to warn) compare `depth` against
    /// [`sets().len()`](Self::sets).
    pub fn select_profile(&self, depth: usize) -> Result<&ProfileSet> {
        let idx = self.set_index(depth).ok_or_else(no_profile_configured)?;
        Ok(self
            .sets
            .values()
            .nth(idx)
            .expect("index derived from the map length"))
    }

    /// Same resolution as [`select_profile`](Self::select_profile), but also returns the
    /// resolved set's positional index (0-based, over the sorted names) so callers that
    /// surface a positional handle can carry the pair without re-deriving it. The pair is
    /// taken atomically at resolution time — after a registry swap but before an apply, a
    /// live agent still holds its old (index, set) pair, which is what its client is using.
    pub fn select_tier(&self, depth: usize) -> Result<(usize, &ProfileSet)> {
        let idx = self.set_index(depth).ok_or_else(no_profile_configured)?;
        let set = self
            .sets
            .values()
            .nth(idx)
            .expect("index derived from the map length");
        Ok((idx, set))
    }

    /// The clamped positional index for `depth` over the sorted set names.
    fn set_index(&self, depth: usize) -> Option<usize> {
        if self.sets.is_empty() {
            return None;
        }
        Some(depth.min(self.sets.len() - 1))
    }

    /// Build a [`ChatClient`] for a profile, looking up its provider's backend via the
    /// [`BackendSource`] (pre-built for the active set, lazily constructed for failover profiles).
    pub fn build_client(
        &self,
        profile: &Profile,
        system_prompt: Option<String>,
    ) -> Result<ChatClient> {
        let backend = self.source.get(&profile.endpoint).with_context(|| {
            format!(
                "profile '{}' references endpoint '{}' with no backend",
                profile.id, profile.endpoint
            )
        })?;

        let mut options = ChatClientOptions::new(profile.model.clone());
        if let Some(system_prompt) = system_prompt {
            options = options.with_system_prompt(system_prompt);
        }
        Ok(ChatClient::new(backend, options))
    }
}

/// Hint surfaced by `select_*` on an empty registry (and reused verbatim by the
/// tagma's sentinel backend, so the zero-profile story reads one voice).
pub const NO_PROFILE_HINT: &str =
    "no model profile is configured; add one via the tagma management page";

fn no_profile_configured() -> anyhow::Error {
    anyhow::anyhow!(NO_PROFILE_HINT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use crate::test_support::{MapSource, ds_backend};

    fn set(name: &str, model: &str) -> ProfileSet {
        ProfileSet {
            name: name.into(),
            description: None,
            profiles: vec![Profile {
                id: format!("p-{name}"),
                endpoint: "ds".into(),
                model: model.into(),
                max_context_window: 500_000,
            }],
        }
    }

    fn single_set_registry() -> ProfileRegistry {
        let mut backends = HashMap::new();
        backends.insert("ds".into(), ds_backend());
        ProfileRegistry::new(
            BTreeMap::from([("only".to_string(), set("only", "deepseek-test"))]),
            Arc::new(MapSource(backends)),
        )
        .unwrap()
    }

    /// "alpha" < "beta" in sorted order, so alpha occupies the first positional slot.
    fn two_set_registry() -> ProfileRegistry {
        let mut backends = HashMap::new();
        backends.insert("ds".into(), ds_backend());
        ProfileRegistry::new(
            BTreeMap::from([
                ("beta".to_string(), set("beta", "deepseek-flash")),
                ("alpha".to_string(), set("alpha", "deepseek-pro")),
            ]),
            Arc::new(MapSource(backends)),
        )
        .unwrap()
    }

    #[test]
    fn new_syncs_set_name_with_map_key() {
        let reg = two_set_registry();
        assert_eq!(reg.sets()["alpha"].name, "alpha");
        assert_eq!(reg.sets()["beta"].name, "beta");
    }

    #[test]
    fn select_set_returns_named_set() {
        let reg = two_set_registry();
        let set = reg.select_set("beta").unwrap();
        assert_eq!(set.active_profile().model, "deepseek-flash");
    }

    #[test]
    fn select_set_unknown_name_lists_available() {
        let reg = two_set_registry();
        let err = reg
            .select_set("missing")
            .expect_err("unknown name must error");
        let msg = format!("{err}");
        assert!(msg.contains("'missing'"), "got: {msg}");
        assert!(msg.contains("alpha") && msg.contains("beta"), "got: {msg}");
    }

    #[test]
    fn select_profile_depth_zero_is_first_sorted_set() {
        let reg = two_set_registry();
        let set = reg.select_profile(0).unwrap();
        assert_eq!(set.name, "alpha");
        assert_eq!(set.active_profile().model, "deepseek-pro");
    }

    #[test]
    fn select_profile_depth_routes_and_clamps() {
        let reg = two_set_registry();
        assert_eq!(
            reg.select_profile(0).unwrap().active_profile().model,
            "deepseek-pro"
        );
        assert_eq!(
            reg.select_profile(1).unwrap().active_profile().model,
            "deepseek-flash"
        );
        assert_eq!(
            reg.select_profile(9).unwrap().active_profile().model,
            "deepseek-flash"
        );
    }

    #[test]
    fn select_tier_returns_index_matching_select_profile() {
        let reg = two_set_registry();
        for depth in [0, 1, 9] {
            let (idx, set) = reg.select_tier(depth).unwrap();
            assert_eq!(
                set.active_profile().model,
                reg.select_profile(depth).unwrap().active_profile().model
            );
            let clamped = depth.min(reg.sets().len() - 1);
            assert_eq!(idx, clamped);
        }
    }

    #[test]
    fn build_client_binds_model_and_system_prompt() {
        let reg = single_set_registry();
        let p = reg.select_set("only").unwrap().active_profile().clone();
        let client = reg.build_client(&p, Some("sp".into())).unwrap();
        assert_eq!(client.model(), "deepseek-test");
        assert_eq!(client.system_prompt(), Some("sp"));
    }

    #[test]
    fn new_allows_empty_collection_and_select_errors() {
        let reg = ProfileRegistry::new(BTreeMap::new(), Arc::new(MapSource(HashMap::new())))
            .expect("empty collection is constructible");
        let err = reg
            .select_tier(0)
            .expect_err("select on an empty registry errors");
        assert!(
            format!("{err}").contains("management page"),
            "error should point at the management page, got: {err}"
        );
        assert!(reg.select_profile(0).is_err());
        assert!(reg.select_set("any").is_err());
    }

    #[test]
    fn new_rejects_set_with_no_profiles() {
        let res = ProfileRegistry::new(
            BTreeMap::from([(
                "empty".to_string(),
                ProfileSet {
                    name: "empty".into(),
                    description: None,
                    profiles: vec![],
                },
            )]),
            Arc::new(MapSource(HashMap::new())),
        );
        let err = res.err().expect("empty set must be rejected");
        assert!(format!("{err}").contains("'empty'"), "got: {err}");
    }

    #[test]
    fn build_client_errors_when_provider_missing() {
        // Provider existence is tagma-validated at startup; this covers the runtime lookup path.
        let mut lone = set("lone", "m");
        lone.profiles[0].endpoint = "missing".into();
        lone.profiles[0].max_context_window = 1000;
        let reg = ProfileRegistry::new(
            BTreeMap::from([("lone".to_string(), lone)]),
            Arc::new(MapSource(HashMap::new())),
        )
        .unwrap();
        let profile = reg.select_set("lone").unwrap().active_profile().clone();
        let err = reg
            .build_client(&profile, None)
            .expect_err("build_client should error on a missing provider");
        assert!(format!("{err}").contains("no backend"), "got: {err}");
    }
}

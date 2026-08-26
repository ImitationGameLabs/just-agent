//! Profile configuration: a TOML file (multi-tier) or an implicit single profile from
//! `KALLIP_LLM_*` env (the no-config-file path).
//!
//! Progressive disclosure — Harbor / `kallip-run` set only env vars and ship no config
//! file, so they get the implicit single profile with zero overhead. A `profiles.toml`
//! unlocks multi-tier / multi-profile failover. Both paths carry a declared `max_context_window`
//! (the implicit profile derives it from `KALLIP_CONTEXT_WINDOW_TOKENS`).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use just_llm_client::family;
use serde::{Deserialize, Serialize};

use super::model::{Profile, Provider, Tier};

/// Parsed + validated profile configuration: the data the tagma assembles into a
/// [`super::registry::ProfileRegistry`] after building backends. Pure data — no reqwest, no
/// backends. The tagma owns construction (see `kallip_runtime::profile`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileConfig {
    /// Ordered capability tiers (selection reads `tiers[depth]`).
    pub tiers: Vec<Tier>,
    /// Named provider instances keyed by [`Provider::id`].
    pub endpoints: HashMap<String, Provider>,
    /// Profiles parked out of rotation: draft space the runtime never
    /// reads (selection is tiers-only), kept so a parked profile survives
    /// without a tier. Empty on old configs (serde default) and absent from
    /// the serialized file when empty (old-file shape unchanged).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parking: Vec<Profile>,
}

/// Load profile configuration: from `<data_dir>/profiles/profiles.toml` when
/// present, else an implicit single profile built from `KALLIP_LLM_*` env.
pub fn load() -> Result<ProfileConfig> {
    match resolve_config_path()? {
        Some(path) => load_file(&path),
        None => from_env(),
    }
}

/// Build the implicit single-profile registry from `KALLIP_LLM_*` env (the env path).
/// With no KALLIP_LLM_PROVIDER set, returns an empty config so the tagma
/// boots profile-less; the management page then adds the first profile.
///
/// The profile's `max_context_window` is derived from `KALLIP_CONTEXT_WINDOW_TOKENS`
/// (default `128_000`), so the env path and the config-file path both carry an authoritative
/// window installed via `set_context_window` at spawn.
pub fn from_env() -> Result<ProfileConfig> {
    // An unset provider boots the empty registry; a set-but-incomplete
    // provider spec falls through to the hard errors below (fail loud on
    // half-configuration, not a silent empty profile).
    let Ok(provider) = std::env::var("KALLIP_LLM_PROVIDER") else {
        return Ok(ProfileConfig {
            tiers: Vec::new(),
            endpoints: HashMap::new(),
            parking: Vec::new(),
        });
    };
    let model = env_str("KALLIP_LLM_MODEL")?;
    let (family_id, api_key, base_url) = match provider.as_str() {
        family::DEEPSEEK => {
            let key = env_str("KALLIP_LLM_DEEPSEEK_API_KEY")?;
            let base = std::env::var("KALLIP_LLM_DEEPSEEK_BASE_URL").ok();
            (family::DEEPSEEK, key, base)
        }
        family::OPENAI_COMPATIBLE => {
            let key = env_str("KALLIP_LLM_OPENAI_COMPAT_API_KEY")?;
            let base = std::env::var("KALLIP_LLM_OPENAI_COMPAT_BASE_URL").ok();
            (family::OPENAI_COMPATIBLE, key, base)
        }
        other => bail!("unsupported KALLIP_LLM_PROVIDER: {other}"),
    };
    let implicit_provider = Provider {
        id: provider.clone(),
        family: family_id.into(),
        api_key,
        base_url,
    };
    // The implicit profile's window comes from the same env var `AgentConfig::load` uses as its
    // budget-shape validation anchor — single source, no drift under static tagma env.
    let max_context_window = crate::env_util::parse_env::<usize>("KALLIP_CONTEXT_WINDOW_TOKENS")?
        .unwrap_or(crate::env_util::DEFAULT_CONTEXT_WINDOW_TOKENS);
    let profile = Profile {
        id: format!("{provider}/{model}"),
        endpoint: provider.clone(),
        model,
        max_context_window,
    };
    let mut endpoints = HashMap::new();
    endpoints.insert(provider, implicit_provider);
    Ok(ProfileConfig {
        tiers: vec![Tier {
            profiles: vec![profile],
        }],
        endpoints,
        parking: vec![],
    })
}

fn load_file(path: &Path) -> Result<ProfileConfig> {
    check_file_mode(path);
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read profiles config {}", path.display()))?;
    let file: ConfigFile = toml::from_str(&raw)
        .with_context(|| format!("failed to parse profiles config {}", path.display()))?;
    validate(&file)?;

    let endpoints: HashMap<String, Provider> = file
        .endpoints
        .into_iter()
        .map(|(id, body)| {
            let api_key = expand_vars(&body.api_key)?;
            // Re-check post-expansion: `${VAR}` that resolves to empty must not slip through
            // (the pre-expansion check in `validate` only sees the literal).
            if api_key.trim().is_empty() {
                bail!("endpoint '{id}': api_key is required");
            }
            let endpoint = Provider {
                id: id.clone(),
                family: body.family,
                api_key,
                base_url: body.base_url.map(|s| expand_vars(&s)).transpose()?,
            };
            Ok::<_, anyhow::Error>((id, endpoint))
        })
        .collect::<Result<_>>()?;

    // Tier/Profile deserialize directly (fields identical, no id issues — profiles
    // carry their id inline); no pass-through mirror types needed.
    let tiers = file.tiers;

    Ok(ProfileConfig {
        tiers,
        endpoints,
        parking: file.parking,
    })
}

/// Resolve the config file path: `<$KALLIP_DATA_DIR>/profiles/profiles.toml` --
/// the same per-instance root `agents/` and `skills/` live under
/// (`data_dir_root`). The data dir is REQUIRED: a bare run without one is a
/// configuration error, not a silent fall-back to a HOME-level file (the old
/// HOME tier shared one file across daemon-spawned instances). Returns `None`
/// when the resolved file does not exist.
fn resolve_config_path() -> Result<Option<PathBuf>> {
    let Some(path) = data_dir_profile_path() else {
        bail!("KALLIP_DATA_DIR is not set; cannot resolve profiles config path");
    };
    Ok(path.exists().then_some(path))
}

/// Resolve the config file path for writing, same single location as the read
/// side. Unlike the read side (which returns `None` when the file does not
/// exist), this always returns a path — `save()` needs a target even on first
/// write. Errors when `KALLIP_DATA_DIR` is unset.
pub fn config_path() -> Result<PathBuf> {
    let Some(path) = data_dir_profile_path() else {
        bail!("KALLIP_DATA_DIR is not set; cannot resolve profiles config path");
    };
    Ok(path)
}
/// The single profiles location shared by both resolve fns:
/// `<data dir>/profiles/profiles.toml` (the same root
/// `persistence::data_dir_root` names, so profiles stay inside the instance's
/// own data tree). `None` when `KALLIP_DATA_DIR` is unset.
fn data_dir_profile_path() -> Option<PathBuf> {
    std::env::var_os("KALLIP_DATA_DIR")
        .map(|d| PathBuf::from(d).join("profiles").join("profiles.toml"))
}

/// Serialize a [`ProfileConfig`] to TOML and write it to `path` atomically
/// (temp file + rename), chmod 600. The `id` field inside each endpoint is
/// redundant with the map key but harmless: [`load_file`] uses an intermediate
/// type that ignores it.
pub fn save(config: &ProfileConfig, path: &Path) -> Result<()> {
    let toml =
        toml::to_string_pretty(config).context("failed to serialize profiles config to TOML")?;
    let parent = path
        .parent()
        .context("profiles config path has no parent directory")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("failed to create config dir {}", parent.display()))?;
    // Write to a temp file in the same directory, then rename for atomicity.
    let tmp = path.with_extension(format!("toml.tmp.{}", std::process::id()));
    std::fs::write(&tmp, &toml)
        .with_context(|| format!("failed to write profiles config to {}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("failed to chmod {}", tmp.display()))?;
    }
    std::fs::rename(&tmp, path)
        .with_context(|| format!("failed to rename temp config to {}", path.display()))?;
    Ok(())
}

/// The directory holding `profiles.toml` (and thus potentially API keys) — the
/// path a sandbox hide-hole should overlay so a broad-read agent cannot read
/// credentials. Mirrors the loader's single location: the dedicated
/// `<data dir>/profiles/` subdir when `KALLIP_DATA_DIR` is set; `None`
/// otherwise (no data dir means no profiles file to hide).
pub fn profiles_config_dir() -> Option<PathBuf> {
    // Same single source as the loader (`data_dir_profile_path`): the
    // hide-hole must follow the resolver, or a relocated profiles.toml leaks
    // past the Guest sandbox. The subdir (a directory, as the tmpfs overlay
    // contract requires) rather than the data root — hiding the root would
    // also hide agents/skills and break Guest agents.
    data_dir_profile_path().and_then(|p| p.parent().map(Path::to_path_buf))
}

/// Warn (non-fatal) if the config file is readable by group/other — it holds API keys.
fn check_file_mode(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mode = meta.permissions().mode();
            if mode & 0o077 != 0 {
                tracing::warn!(
                    path = %path.display(),
                    mode = format!("{mode:o}"),
                    "profiles config is group/other-accessible but may contain API keys; \
                     recommend chmod 600"
                );
            }
        }
    }
}

/// Expand `${VAR}` references against the process environment.
///
/// Intentionally minimal: literal `${VAR}` substitution only, applied to operator-controlled config
/// values (`api_key`, `base_url` in `profiles.toml`) — no `$$` escaping, no default values, no
/// nested/recursive expansion, and an unset or unterminated `${` is a hard error (config validation,
/// not silent substitution). There is no injection surface: the inputs are operator config, never
/// agent/LLM/user content. Pulling a full templating crate would trade this simple, fail-loud
/// contract for incidental features.
fn expand_vars(s: &str) -> Result<String> {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(open) = rest.find("${") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        let Some(close) = after.find('}') else {
            bail!("unterminated ${{ in config value: {s:?}");
        };
        let name = &after[..close];
        let val = std::env::var(name)
            .with_context(|| format!("config references unset env var ${{{name}}}"))?;
        out.push_str(&val);
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

fn env_str(name: &str) -> Result<String> {
    std::env::var(name).with_context(|| format!("{name} must be set"))
}

/// Validate the parsed file: non-empty `api_key`, unique profile ids across
/// tiers ∪ parking (both hold the same id namespace — a duplicate would break
/// the wire-boundary invariant on the next PUT). Profile→endpoint references and
/// backend coverage are validated when the tagma constructs `ProfileRegistry`
/// (tiers only — parked profiles may dangle by design).
fn validate(file: &ConfigFile) -> Result<()> {
    for (id, body) in &file.endpoints {
        if body.api_key.trim().is_empty() {
            bail!("endpoint '{id}': api_key is required");
        }
    }
    let mut seen: HashSet<&str> = HashSet::new();
    for tier in &file.tiers {
        for p in &tier.profiles {
            if !seen.insert(p.id.as_str()) {
                bail!("duplicate profile id '{}'", p.id);
            }
        }
    }
    for p in &file.parking {
        if !seen.insert(p.id.as_str()) {
            bail!("duplicate profile id '{}'", p.id);
        }
    }
    Ok(())
}

// --- serde-facing types (TOML schema) ---

#[derive(Deserialize)]
struct ConfigFile {
    #[serde(default)]
    endpoints: HashMap<String, ProviderEntry>,
    #[serde(default)]
    tiers: Vec<Tier>,
    #[serde(default)]
    parking: Vec<Profile>,
}

#[derive(Deserialize)]
struct ProviderEntry {
    family: String,
    api_key: String,
    #[serde(default)]
    base_url: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ds_env() -> [(&'static str, Option<&'static str>); 4] {
        [
            ("KALLIP_LLM_PROVIDER", Some("deepseek")),
            ("KALLIP_LLM_MODEL", Some("deepseek-test")),
            ("KALLIP_LLM_DEEPSEEK_API_KEY", Some("fake")),
            ("KALLIP_CONTEXT_WINDOW_TOKENS", Some("200000")),
        ]
    }

    #[test]
    fn from_env_builds_implicit_single_profile() {
        temp_env::with_vars(ds_env(), || {
            let cfg = from_env().unwrap();
            // Env path yields a single implicit tier.
            let p = &cfg.tiers[0].profiles[0];
            assert_eq!(p.model, "deepseek-test");
            assert_eq!(p.max_context_window, 200_000); // implicit env profile derives the window from the env var
            assert!(cfg.parking.is_empty()); // env path has no draft space
        });
    }

    #[test]
    fn from_env_without_provider_boots_empty() {
        temp_env::with_vars([("KALLIP_LLM_PROVIDER", None::<&str>)], || {
            let cfg = from_env().unwrap();
            assert!(cfg.tiers.is_empty());
            assert!(cfg.endpoints.is_empty());
            assert!(cfg.parking.is_empty());
        });
    }

    #[test]
    fn from_env_half_configured_fails_loud() {
        temp_env::with_vars(
            [
                ("KALLIP_LLM_PROVIDER", Some("deepseek")),
                ("KALLIP_LLM_MODEL", None),
            ],
            || {
                let err = from_env().unwrap_err();
                assert!(format!("{err:#}").contains("KALLIP_LLM_MODEL"));
            },
        );
    }

    #[test]
    fn from_env_rejects_unknown_provider() {
        temp_env::with_vars(
            [
                ("KALLIP_LLM_PROVIDER", Some("anthropic")),
                ("KALLIP_LLM_MODEL", Some("m")),
            ],
            || {
                assert!(from_env().is_err());
            },
        );
    }

    #[test]
    fn parse_valid_toml() {
        let toml = r#"
[endpoints.ds]
family = "deepseek"
api_key = "fake"

[[tiers]]

  [[tiers.profiles]]
  id = "pro"
  endpoint = "ds"
  model = "deepseek-pro"
  max_context_window = 500000
"#;
        let file: ConfigFile = toml::from_str(toml).unwrap();
        validate(&file).unwrap();
    }

    #[test]
    fn parse_rejects_duplicate_profile_id() {
        let toml = r#"
[endpoints.ds]
family = "deepseek"
api_key = "fake"
[[tiers]]

  [[tiers.profiles]]
  id = "dup"
  endpoint = "ds"
  model = "m"
  max_context_window = 1000
  [[tiers.profiles]]
  id = "dup"
  endpoint = "ds"
  model = "m2"
  max_context_window = 1000
"#;
        let file: ConfigFile = toml::from_str(toml).unwrap();
        assert!(validate(&file).is_err());
    }

    #[test]
    fn expand_vars_substitutes_env() {
        temp_env::with_vars([("MY_SECRET", Some("shhh"))], || {
            assert_eq!(
                expand_vars("prefix-${MY_SECRET}-suffix").unwrap(),
                "prefix-shhh-suffix"
            );
        });
    }

    #[test]
    fn expand_vars_errors_on_unset() {
        temp_env::with_vars_unset(["DEFINITELY_UNSET_VAR_X9Q"], || {
            assert!(expand_vars("${DEFINITELY_UNSET_VAR_X9Q}").is_err());
        });
    }

    #[test]
    fn save_load_roundtrip() {
        use super::{Profile, Provider, Tier};
        use std::collections::HashMap;

        let mut endpoints = HashMap::new();
        endpoints.insert(
            "ds".into(),
            Provider {
                id: "ds".into(),
                family: "deepseek".into(),
                api_key: "secret-key".into(),
                base_url: None,
            },
        );
        endpoints.insert(
            "oa".into(),
            Provider {
                id: "oa".into(),
                family: "openai-compatible".into(),
                api_key: "other-key".into(),
                base_url: Some("https://api.example.com".into()),
            },
        );
        let original = ProfileConfig {
            tiers: vec![Tier {
                profiles: vec![
                    Profile {
                        id: "pro".into(),
                        endpoint: "ds".into(),
                        model: "deepseek-pro".into(),
                        max_context_window: 500_000,
                    },
                    Profile {
                        id: "backup".into(),
                        endpoint: "oa".into(),
                        model: "gpt-4".into(),
                        max_context_window: 128_000,
                    },
                ],
            }],
            endpoints,
            // A parked profile rides along (out of rotation, may dangle).
            parking: vec![Profile {
                id: "spare".into(),
                endpoint: "oa".into(),
                model: "gpt-4-mini".into(),
                max_context_window: 128_000,
            }],
        };

        // Serialize to TOML
        let toml_str = toml::to_string_pretty(&original).expect("serialize");
        // Parse back via load_file's internal types (simulates disk round-trip)
        let file: ConfigFile = toml::from_str(&toml_str).expect("parse back");
        validate(&file).expect("validate");

        // Verify the parsed data matches
        assert_eq!(file.tiers.len(), 1);
        assert_eq!(file.tiers[0].profiles.len(), 2);
        assert_eq!(file.tiers[0].profiles[0].id, "pro");
        assert_eq!(file.tiers[0].profiles[0].endpoint, "ds");
        assert_eq!(file.endpoints.len(), 2);
        assert_eq!(file.endpoints["ds"].family, "deepseek");
        assert_eq!(file.endpoints["ds"].api_key, "secret-key");
        assert_eq!(
            file.endpoints["oa"].base_url.as_deref(),
            Some("https://api.example.com")
        );
        assert_eq!(file.parking.len(), 1);
        assert_eq!(file.parking[0].id, "spare");
    }

    #[test]
    fn save_empty_parking_writes_no_key() {
        use super::{Profile, Provider, Tier};
        use std::collections::HashMap;

        let mut endpoints = HashMap::new();
        endpoints.insert(
            "ds".into(),
            Provider {
                id: "ds".into(),
                family: "deepseek".into(),
                api_key: "k".into(),
                base_url: None,
            },
        );
        let cfg = ProfileConfig {
            tiers: vec![Tier {
                profiles: vec![Profile {
                    id: "pro".into(),
                    endpoint: "ds".into(),
                    model: "m".into(),
                    max_context_window: 1000,
                }],
            }],
            endpoints,
            parking: vec![],
        };
        let toml_str = toml::to_string_pretty(&cfg).expect("serialize");
        assert!(
            !toml_str.contains("parking"),
            "empty parking must not write a key (old-file shape), got: {toml_str}"
        );
    }

    #[test]
    fn parse_old_toml_without_parking_loads_empty() {
        let toml = r#"
[endpoints.ds]
family = "deepseek"
api_key = "fake"
[[tiers]]

  [[tiers.profiles]]
  id = "pro"
  endpoint = "ds"
  model = "deepseek-pro"
  max_context_window = 500000
"#;
        let file: ConfigFile = toml::from_str(toml).unwrap();
        validate(&file).unwrap();
        assert!(file.parking.is_empty());
    }

    #[test]
    fn parse_rejects_duplicate_id_between_tier_and_parking() {
        let toml = r#"
[endpoints.ds]
family = "deepseek"
api_key = "fake"
[[tiers]]

  [[tiers.profiles]]
  id = "dup"
  endpoint = "ds"
  model = "m"
  max_context_window = 1000

[[parking]]
id = "dup"
endpoint = "ds"
model = "m2"
max_context_window = 1000
"#;
        let file: ConfigFile = toml::from_str(toml).unwrap();
        assert!(validate(&file).is_err());
    }

    #[test]
    fn data_dir_is_the_only_profiles_location() {
        let tmp = tempfile::tempdir().unwrap();
        let data_profiles = tmp.path().join("profiles").join("profiles.toml");
        std::fs::create_dir_all(data_profiles.parent().unwrap()).unwrap();
        std::fs::write(&data_profiles, "\n").unwrap();
        temp_env::with_vars(
            [("KALLIP_DATA_DIR", Some(tmp.path().to_str().unwrap()))],
            || {
                assert_eq!(
                    resolve_config_path().unwrap().as_deref(),
                    Some(data_profiles.as_path())
                );
                assert_eq!(config_path().unwrap(), data_profiles);
            },
        );
    }

    #[test]
    fn missing_data_dir_is_an_error_not_a_home_fall_back() {
        temp_env::with_vars_unset(["KALLIP_DATA_DIR"], || {
            // The old HOME tier silently shared one file across instances;
            // a bare run without a data dir must fail loud instead.
            assert!(resolve_config_path().is_err());
            assert!(config_path().is_err());
            assert!(profiles_config_dir().is_none());
        });
    }

    #[test]
    fn data_dir_reads_only_when_file_exists() {
        let tmp = tempfile::tempdir().unwrap();
        temp_env::with_vars(
            [("KALLIP_DATA_DIR", Some(tmp.path().to_str().unwrap()))],
            || {
                // No profiles.toml in the data dir: the read side must yield
                // None, not fall through to any other location.
                let resolved = resolve_config_path().unwrap();
                assert!(resolved.is_none());
            },
        );
    }

    #[test]
    fn profiles_config_dir_follows_the_data_dir() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("profiles")).unwrap();
        temp_env::with_vars(
            [("KALLIP_DATA_DIR", Some(tmp.path().to_str().unwrap()))],
            || {
                // The hide-hole source must track the resolver: it hides
                // the dedicated profiles/ subdir (a directory), not the data
                // root (agents/skills stay Guest-visible).
                assert_eq!(
                    profiles_config_dir().as_deref(),
                    Some(tmp.path().join("profiles").as_path())
                );
            },
        );
    }
}

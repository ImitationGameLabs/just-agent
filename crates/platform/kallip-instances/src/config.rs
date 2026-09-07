//! Startup configuration: bind address, daemon socket, and the
//! auth-mode inputs (platform credentials or a standalone token).

use std::path::PathBuf;

use clap::Parser;

/// Local instance management service for the kallip daemon: proxies
/// `/api/instances/*` to the daemon's UDS socket.
#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Config {
    /// Listen address. Loopback by default; a LAN deployment points this at
    /// the machine's LAN address explicitly.
    #[arg(long, env = "KALLIP_INSTANCES_ADDR", default_value = "127.0.0.1:7300")]
    pub addr: String,

    /// Daemon control socket. Defaults to the same XDG-derived path the
    /// daemon itself uses when KALLIP_DAEMON_SOCKET is unset (deliberate
    /// duplication: the daemon crate is a binary, so this mirrors its
    /// main.rs instead of sharing code).
    #[arg(long, env = "KALLIP_DAEMON_SOCKET")]
    pub daemon_socket: Option<PathBuf>,

    /// Standalone-mode bearer token for /api/instances/* (constant-time
    /// compared; the platform mode uses the archeion credentials below).
    #[arg(long, env = "KALLIP_INSTANCES_TOKEN")]
    pub token: Option<String>,
    /// Instance source: the host daemon (the only implementation today;
    /// a cloud orchestration backend is reserved but not built yet).
    #[arg(long, env = "KALLIP_INSTANCES_BACKEND", default_value = "daemon")]
    pub backend: String,
    /// Archeion internal root for platform mode (e.g. http://127.0.0.1:7100);
    /// together with the internal token this enables archeion-backed auth.
    #[arg(long, env = "KALLIP_INSTANCES_ARCHEION_URL")]
    pub archeion_internal_url: Option<String>,

    /// Shared platform-internal secret (KALLIP_POLIS_INTERNAL_TOKEN).
    #[arg(long, env = "KALLIP_POLIS_INTERNAL_TOKEN")]
    pub archeion_internal_token: Option<String>,
    /// Server-side default relay URLs for spawned tagmata that signal
    /// relay intent (any KALLIP_TAGMA_RELAY_* env) but omit a URL; local-
    /// backend scope only (the daemon and its tagmata run on this host,
    /// so localhost always reaches the local stack).
    #[arg(
        long,
        env = "KALLIP_INSTANCES_RELAY_ARCHEION_URL",
        default_value = "http://localhost:7100"
    )]
    pub relay_archeion_url: String,
    /// Lesche counterpart of `relay_archeion_url` (tunnel + envelopes).
    #[arg(
        long,
        env = "KALLIP_INSTANCES_RELAY_LESCHE_URL",
        default_value = "http://localhost:7200"
    )]
    pub relay_lesche_url: String,

    /// Extra Host values allowed through the host guard (comma separated;
    /// a reverse-proxied deployment names its public domain here).
    #[arg(long, env = "KALLIP_INSTANCES_ALLOWED_HOSTS", default_value = "")]
    pub allowed_hosts_raw: String,
    /// Comma-separated CORS allowed origins (the app's origin(s)). Empty
    /// = no cross-origin allowed. Never use a wildcard on a public-facing
    /// deploy.
    #[arg(long, env = "KALLIP_INSTANCES_CORS_ORIGINS", default_value = "")]
    pub cors_origins: String,
}

impl Config {
    /// Resolve the daemon socket by probing the shared chain in order
    /// (see `kallip_daemon_common::socket`): the configured socket verbatim
    /// first, then the environment legs - the first socket that answers
    /// is wherever the daemon actually bound.
    pub fn resolve_socket(&self) -> anyhow::Result<PathBuf> {
        let candidates = kallip_daemon_common::socket::candidates_from_env(
            self.daemon_socket.as_deref().map(std::path::Path::new),
        );
        kallip_daemon_common::socket::probe(&candidates).ok_or_else(|| {
            anyhow::anyhow!(
                "no reachable daemon socket; tried: {}",
                candidates
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
    }

    /// The parsed allowlist from `allowed_hosts_raw` (empty = none).
    pub fn allowed_hosts(&self) -> Vec<String> {
        self.allowed_hosts_raw
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bind a listener at `path` (parent dirs created); the sandboxed
    /// stand-in for a live daemon socket.
    fn live_socket(path: &std::path::Path) -> std::os::unix::net::UnixListener {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::os::unix::net::UnixListener::bind(path).unwrap()
    }
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Set/clear env keys for the closure's duration. Serial under
    /// ENV_LOCK; mirrors the daemon-common socket tests.
    fn with_env<R>(vars: &[(&str, Option<&std::ffi::OsStr>)], f: impl FnOnce() -> R) -> R {
        let _guard = ENV_LOCK.lock().unwrap();
        let mut applied: Vec<(&str, Option<std::ffi::OsString>)> = Vec::new();
        for (key, value) in vars {
            applied.push((key, std::env::var_os(key)));
            match value {
                Some(v) => unsafe { std::env::set_var(key, v) },
                None => unsafe { std::env::remove_var(key) },
            }
        }
        let out = f();
        for (key, old) in applied {
            match old {
                Some(v) => unsafe { std::env::set_var(key, v) },
                None => unsafe { std::env::remove_var(key) },
            }
        }
        out
    }

    #[test]
    fn flag_socket_wins() {
        // Pin the env legs so this env-reading test cannot race the
        // lock-holding writer in the sibling test; the explicit flag
        // must win over the pinned (empty) legs.
        let scratch = tempfile::tempdir().unwrap();
        let env_runtime = tempfile::tempdir().unwrap();
        let env_state = tempfile::tempdir().unwrap();
        let flag_path = scratch.path().join("flag.sock");
        let _listener = live_socket(&flag_path);
        with_env(
            &[
                ("XDG_RUNTIME_DIR", Some(env_runtime.path().as_os_str())),
                ("XDG_STATE_HOME", Some(env_state.path().as_os_str())),
                ("KALLIP_DAEMON_SOCKET", None),
            ],
            || {
                let config = Config {
                    addr: "127.0.0.1:7300".into(),
                    daemon_socket: Some(flag_path.clone()),
                    token: None,
                    backend: "daemon".into(),
                    relay_archeion_url: String::new(),
                    relay_lesche_url: String::new(),
                    archeion_internal_url: None,
                    archeion_internal_token: None,
                    allowed_hosts_raw: String::new(),
                    cors_origins: String::new(),
                };
                assert_eq!(config.resolve_socket().expect("resolve"), flag_path);
            },
        );
    }

    #[test]
    fn no_reachable_socket_names_the_tried_candidates() {
        // Pin the environment legs at empty scratch trees so a live
        // daemon elsewhere on the host cannot answer the probe.
        let runtime = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        with_env(
            &[
                ("XDG_RUNTIME_DIR", Some(runtime.path().as_os_str())),
                ("XDG_STATE_HOME", Some(state.path().as_os_str())),
                ("KALLIP_DAEMON_SOCKET", None),
            ],
            || {
                let config = Config {
                    addr: "127.0.0.1:7300".into(),
                    daemon_socket: None,
                    token: None,
                    backend: "daemon".into(),
                    relay_archeion_url: String::new(),
                    relay_lesche_url: String::new(),
                    archeion_internal_url: None,
                    archeion_internal_token: None,
                    allowed_hosts_raw: String::new(),
                    cors_origins: String::new(),
                };
                let err = config.resolve_socket().unwrap_err();
                assert!(
                    err.to_string().contains("no reachable daemon socket"),
                    "{err}"
                );
            },
        );
    }

    #[test]
    fn allowed_hosts_splits_and_trims() {
        let config = Config {
            addr: "127.0.0.1:7300".into(),
            daemon_socket: None,
            token: None,
            backend: "daemon".into(),
            relay_archeion_url: String::new(),
            relay_lesche_url: String::new(),
            archeion_internal_url: None,
            archeion_internal_token: None,
            allowed_hosts_raw: " platform.internal , localhost ,".into(),
            cors_origins: String::new(),
        };
        assert_eq!(
            config.allowed_hosts(),
            vec!["platform.internal", "localhost"]
        );
    }
}

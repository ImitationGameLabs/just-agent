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

    /// Shared secret matching the archeion's KALLIP_ARCHEION_INTERNAL_TOKEN.
    #[arg(long, env = "KALLIP_INSTANCES_ARCHEION_INTERNAL_TOKEN")]
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
    /// Resolve the daemon socket path, mirroring the daemon's own chain:
    /// the KALLIP_DAEMON_SOCKET flag verbatim, else KALLIP_STATE_DIR +
    /// control.sock (a system unit points both processes at that env
    /// pair), else the XDG state home default. Fails hard when the
    /// platform state root cannot be determined, like the daemon.
    pub fn resolve_socket(&self) -> anyhow::Result<PathBuf> {
        if let Some(path) = &self.daemon_socket {
            return Ok(path.clone());
        }
        if let Some(dir) = std::env::var_os("KALLIP_STATE_DIR") {
            return Ok(PathBuf::from(dir).join("control.sock"));
        }
        Ok(dirs::state_dir()
            .ok_or_else(|| anyhow::anyhow!("could not determine the platform state directory"))?
            .join("kallip-daemon")
            .join("control.sock"))
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

    #[test]
    fn flag_socket_wins() {
        let config = Config {
            addr: "127.0.0.1:7300".into(),
            daemon_socket: Some(PathBuf::from("/tmp/flag.sock")),
            token: None,
            backend: "daemon".into(),
            relay_archeion_url: String::new(),
            relay_lesche_url: String::new(),
            archeion_internal_url: None,
            archeion_internal_token: None,
            allowed_hosts_raw: String::new(),
            cors_origins: String::new(),
        };
        assert_eq!(
            config.resolve_socket().expect("resolve"),
            PathBuf::from("/tmp/flag.sock")
        );
    }

    #[test]
    fn default_socket_lands_in_the_daemon_state_dir() {
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
        let socket = config.resolve_socket().expect("resolve");
        // Only the shape is asserted: the XDG root varies by environment.
        assert!(socket.ends_with("kallip-daemon/control.sock"), "{socket:?}");
    }
    #[test]
    fn state_dir_env_honored_between_flag_and_xdg() {
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
        // A system unit points both the daemon and this proxy at the
        // same state dir; skipping this tier is how the paths fork.
        temp_env::with_var("KALLIP_STATE_DIR", Some("/run/kallip-daemon"), || {
            assert_eq!(
                config.resolve_socket().expect("resolve"),
                PathBuf::from("/run/kallip-daemon/control.sock")
            );
        });
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

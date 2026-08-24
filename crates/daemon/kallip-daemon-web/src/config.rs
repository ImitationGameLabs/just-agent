//! Startup configuration: bind address, daemon socket, optional static
//! directory, and the bearer token.

use std::path::PathBuf;

use clap::Parser;

/// Local web management proxy for the kallip daemon: serves the web UI's
/// static build and proxies `/api/daemon/*` to the daemon's UDS socket.
#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Config {
    /// Listen address. Loopback by default; a LAN deployment points this at
    /// the machine's LAN address explicitly.
    #[arg(long, env = "KALLIP_DAEMON_WEB_ADDR", default_value = "127.0.0.1:7300")]
    pub addr: String,

    /// Daemon control socket. Defaults to the same XDG-derived path the
    /// daemon itself uses when KALLIP_DAEMON_SOCKET is unset (deliberate
    /// duplication: the daemon crate is a binary, so this mirrors its
    /// main.rs instead of sharing code).
    #[arg(long, env = "KALLIP_DAEMON_SOCKET")]
    pub daemon_socket: Option<PathBuf>,

    /// Directory of the web UI's static build to serve. Omitted = API-only
    /// mode (dev: vite serves the frontend and proxies /api/daemon here).
    #[arg(long, env = "KALLIP_DAEMON_WEB_STATIC_DIR")]
    pub static_dir: Option<PathBuf>,

    /// Bearer token for /api/daemon/*. When omitted, a token is generated at
    /// startup and printed once to the log for copying into the browser.
    #[arg(long, env = "KALLIP_DAEMON_WEB_TOKEN")]
    pub token: Option<String>,
}

impl Config {
    /// Resolve the daemon socket path: the flag/env verbatim, else the XDG
    /// state home default (`~/.local/state/kallip-daemon/control.sock` on a
    /// rootless Linux), matching the daemon's own fallback.
    pub fn resolve_socket(&self) -> PathBuf {
        if let Some(path) = &self.daemon_socket {
            return path.clone();
        }
        dirs::state_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("kallip-daemon")
            .join("control.sock")
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
            static_dir: None,
            token: None,
        };
        assert_eq!(config.resolve_socket(), PathBuf::from("/tmp/flag.sock"));
    }

    #[test]
    fn default_socket_lands_in_the_daemon_state_dir() {
        let config = Config {
            addr: "127.0.0.1:7300".into(),
            daemon_socket: None,
            static_dir: None,
            token: None,
        };
        let socket = config.resolve_socket();
        // Only the shape is asserted: the XDG root varies by environment.
        assert!(socket.ends_with("kallip-daemon/control.sock"), "{socket:?}");
    }
}

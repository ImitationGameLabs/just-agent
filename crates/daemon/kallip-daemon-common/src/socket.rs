//! Control-socket resolution, shared by the daemon and every client so
//! both sides walk the same legs in the same order.
//!
//! Leg order:
//!
//! 1. an explicit `--socket` (clients only - the daemon has no flag)
//! 2. `KALLIP_DAEMON_SOCKET`
//! 3. `$XDG_RUNTIME_DIR/kallipai/daemon/control.sock`, when the runtime
//!    directory is set and usable
//! 4. the platform state home's `kallipai/daemon/control.sock`
//! 5. `/run/kallipai/daemon.sock` - clients only; the NixOS system
//!    daemon's well-known path
//!
//! The daemon binds the FIRST candidate and never falls through on bind
//! failure - binding leg N while clients probe from the top would split
//! the control plane - and its chain stops at leg 4: a user-process
//! daemon must not grab the system path. Clients probe the candidates
//! in order and connect to the first that answers. Identical ordering
//! plus sequential probing keeps the sides converged: a login-session
//! client probes the RUNTIME leg, finds nothing there, and lands on the
//! daemon's state-home socket. Leg 5 is pure client fallback - on a
//! NixOS host the module exports `KALLIP_DAEMON_SOCKET` (leg 2) already,
//! so the env leg still wins first and leg 5 only serves a bare,
//! zero-config client reaching the system daemon.
use std::path::{Path, PathBuf};

/// One resolution leg's raw inputs, collected so the pure ordering can be
/// tested without touching process state.
pub struct SocketLegs<'a> {
    pub explicit: Option<&'a Path>,
    /// Raw `KALLIP_DAEMON_SOCKET`: set-but-empty is still a set value.
    pub daemon_socket_env: Option<String>,
    /// The runtime-dir leg, already resolved to `None` when
    /// `$XDG_RUNTIME_DIR` is unset, empty, or not an existing directory.
    pub runtime_dir: Option<PathBuf>,
    /// The platform state home's `kallipai/daemon` directory, when the
    /// state home can be determined.
    pub state_default: Option<PathBuf>,
}

/// The NixOS system daemon's well-known socket: the last client leg, so
/// a zero-config client can reach a system-installed daemon. Clients
/// only - `daemon_bind_path` never returns it, because a user-process
/// daemon must not bind the system path. Must stay in sync with the
/// `daemonSocket` binding in `nix/nixos-modules.nix`.
pub const SYSTEM_DAEMON_SOCKET: &str = "/run/kallipai/daemon.sock";

/// Collect the legs from the process environment (and an optional
/// client-side explicit socket).
pub fn legs_from_env(explicit: Option<&Path>) -> SocketLegs<'_> {
    let env = |key: &str| std::env::var(key).ok();
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| !dir.as_os_str().is_empty() && dir.is_dir());
    SocketLegs {
        explicit,
        daemon_socket_env: env("KALLIP_DAEMON_SOCKET"),
        runtime_dir,
        state_default: dirs::state_dir().map(|home| home.join("kallipai").join("daemon")),
    }
}

/// Candidate socket paths in probe order: legs that cannot resolve are
/// skipped, duplicates removed, order preserved.
pub fn candidates(legs: &SocketLegs) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut push = |path: PathBuf| {
        if !out.contains(&path) {
            out.push(path);
        }
    };
    if let Some(explicit) = legs.explicit {
        push(explicit.to_path_buf());
    }
    if let Some(socket) = legs.daemon_socket_env.as_deref().filter(|s| !s.is_empty()) {
        push(PathBuf::from(socket));
    }
    if let Some(runtime) = &legs.runtime_dir {
        push(runtime.join("kallipai").join("daemon").join("control.sock"));
    }
    if let Some(state) = &legs.state_default {
        push(state.join("control.sock"));
    }
    out
}

/// The candidates a client probes: environment chain plus the explicit
/// socket first.
pub fn candidates_from_env(explicit: Option<&Path>) -> Vec<PathBuf> {
    let mut out = candidates(&legs_from_env(explicit));
    let system = PathBuf::from(SYSTEM_DAEMON_SOCKET);
    if !out.contains(&system) {
        out.push(system);
    }
    out
}

/// The daemon's bind path: the first candidate of the environment chain
/// (the daemon has no explicit flag). `None` when no leg resolves - the
/// caller turns that into a pointed error.
pub fn daemon_bind_path() -> Option<PathBuf> {
    candidates(&legs_from_env(None)).into_iter().next()
}

/// The first candidate that accepts a connection. A local unix-socket
/// connect either answers immediately or fails; no timeout is needed.
pub fn probe(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates
        .iter()
        .find(|path| std::os::unix::net::UnixStream::connect(path).is_ok())
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Env mutation is process-global: serialize the env-touching tests.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Set/clear env keys for the closure's duration. Serial under ENV_LOCK.
    fn with_env<R>(vars: &[(&str, Option<&str>)], f: impl FnOnce() -> R) -> R {
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
    fn leg_order_is_explicit_env_runtime_default() {
        let legs = SocketLegs {
            explicit: Some(Path::new("/explicit.sock")),
            daemon_socket_env: Some("/env.sock".into()),
            runtime_dir: Some(PathBuf::from("/run/user/1000")),
            state_default: Some(PathBuf::from("/state-home/kallipai/daemon")),
        };
        assert_eq!(
            candidates(&legs),
            vec![
                PathBuf::from("/explicit.sock"),
                PathBuf::from("/env.sock"),
                PathBuf::from("/run/user/1000/kallipai/daemon/control.sock"),
                PathBuf::from("/state-home/kallipai/daemon/control.sock"),
            ]
        );
    }

    #[test]
    fn unresolvable_legs_are_skipped_and_duplicates_removed() {
        let legs = SocketLegs {
            explicit: Some(Path::new("/env.sock")),
            daemon_socket_env: Some("/env.sock".into()),
            runtime_dir: None,
            state_default: Some(PathBuf::from("/state-home/kallipai/daemon")),
        };
        assert_eq!(
            candidates(&legs),
            vec![
                PathBuf::from("/env.sock"),
                PathBuf::from("/state-home/kallipai/daemon/control.sock"),
            ]
        );
    }

    #[test]
    fn set_but_empty_env_legs_are_skipped() {
        let legs = SocketLegs {
            explicit: None,
            daemon_socket_env: Some(String::new()),
            runtime_dir: None,
            state_default: Some(PathBuf::from("/state-home/kallipai/daemon")),
        };
        assert_eq!(
            candidates(&legs),
            vec![PathBuf::from("/state-home/kallipai/daemon/control.sock")]
        );
    }

    #[test]
    fn client_chain_ends_with_the_system_leg() {
        with_env(
            &[("KALLIP_DAEMON_SOCKET", None), ("XDG_RUNTIME_DIR", None)],
            || {
                let chain = candidates_from_env(None);
                assert_eq!(
                    chain.last().map(|p| p.as_path()),
                    Some(Path::new(SYSTEM_DAEMON_SOCKET))
                );
            },
        );
    }

    #[test]
    fn system_leg_appears_once_even_when_env_points_at_it() {
        with_env(
            &[
                ("KALLIP_DAEMON_SOCKET", Some(SYSTEM_DAEMON_SOCKET)),
                ("XDG_RUNTIME_DIR", None),
            ],
            || {
                let chain = candidates_from_env(None);
                let count = chain
                    .iter()
                    .filter(|p| p.as_os_str() == SYSTEM_DAEMON_SOCKET)
                    .count();
                // The env leg already answers at its own position (which
                // probes first); the appended system leg must not repeat it.
                assert_eq!(count, 1, "env leg and system leg collapse to one");
            },
        );
    }

    #[test]
    fn daemon_bind_chain_excludes_the_system_leg() {
        with_env(
            &[("KALLIP_DAEMON_SOCKET", None), ("XDG_RUNTIME_DIR", None)],
            || {
                // The daemon's chain stops at the state-home leg; the
                // system path is client-only, so it must not appear at any
                // position of the bind order.
                let chain = candidates(&legs_from_env(None));
                assert!(!chain.is_empty(), "daemon bind chain unexpectedly empty");
                assert!(
                    !chain.iter().any(|p| p.as_os_str() == SYSTEM_DAEMON_SOCKET),
                    "system path must not appear in the daemon bind chain"
                );
            },
        );
    }

    #[test]
    fn daemon_binds_the_first_env_candidate() {
        with_env(
            &[
                ("KALLIP_DAEMON_SOCKET", Some("/env.sock")),
                ("XDG_RUNTIME_DIR", None),
            ],
            || {
                assert_eq!(daemon_bind_path(), Some(PathBuf::from("/env.sock")));
            },
        );
    }

    #[test]
    fn probe_walks_candidates_in_order_until_one_answers() {
        let dir = std::env::temp_dir().join(format!("kp-sock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let live = dir.join("live.sock");
        let listener = std::os::unix::net::UnixListener::bind(&live).unwrap();
        let dead = dir.join("dead.sock");

        let found = probe(&[dead.clone(), live.clone()]);
        assert_eq!(found, Some(live), "dead candidate skipped, live answered");
        drop(listener);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

//! kallip-daemon: stateless, directory-driven manager for local kallip
//! instances.
//!
//! Configuration is exactly two environment variables — no config file:
//! - `KALLIP_DAEMON_DATA_DIR`: the instance tree root (default
//!   `~/.local/share/kallipai/tagmata`); each child directory with a
//!   `meta.json` is a managed instance.
//! - `KALLIP_DAEMON_SOCKET`: an explicit control-socket path; without it
//!   the socket binds the first resolvable leg of the shared chain (see
//!   `kallip_daemon_common::socket`): the runtime dir, then the state
//!   home default (`~/.local/state/kallipai/daemon`).
//!
//! The control socket is 0600: filesystem permission is the only auth.

mod bins;
mod reconcile;
mod scan;
mod server;
mod spawn;
mod start;
mod stop;

use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::{Context as _, Result};

fn main() -> Result<()> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    // Deploy-surface observability: log which tagma binary launches will
    // actually exec. Same resolution chain as spawn's bins::resolve
    // (KALLIP_BIN_DIR → current-exe dir → PATH), so the log answers exactly
    // "what will this daemon start". Boot-time snapshot: the PATH branch
    // stays PATH-shaped (the final hit happens at exec time); every other
    // branch is stable for the process lifetime.
    let tagma = bins::resolve("kallip-tagma");
    if tagma
        .parent()
        .is_some_and(|dir| !dir.as_os_str().is_empty())
    {
        tracing::info!(path = %tagma.display(), "resolved kallip-tagma");
    } else {
        tracing::warn!("kallip-tagma unresolved beside the daemon; launches rely on PATH lookup");
    }

    let data_root = data_root()?;
    // Own the tree's existence: the slug-era root sits two segments
    // deeper than the data home, and no deploy step creates it — spawn
    // allocates only the instance leaf, so the trunk must predate it.
    std::fs::create_dir_all(&data_root)
        .with_context(|| format!("create data root {}", data_root.display()))?;
    let socket_path = kallip_daemon_common::socket::daemon_bind_path().context(
        "no control-socket candidate: set KALLIP_DAEMON_SOCKET, or make the platform state home determinable",
    )?;

    // Prove-liveness socket takeover: a blind unlink
    // could steal the endpoint of a live daemon — the first daemon keeps
    // running but loses its socket file. Instead, bind; on EADDRINUSE,
    // probe by connecting: a live listener refuses startup ("already
    // running"), a refused connection means the file is stale, unlink and
    // bind once more.
    // Close the umask before anything is created: the socket is chmod'd
    // 0600 only after it exists, and a group/other-connectable window
    // in between is unacceptable for the system-install layout. Every
    // file this daemon creates is private, so the mask stays for life.
    unsafe { libc::umask(0o077) };
    std::fs::create_dir_all(
        socket_path
            .parent()
            .context("control socket path has no parent")?,
    )
    .context("create control-socket parent dir")?;
    // Prove-liveness check runs on std sockets (blocking is fine for a
    // one-shot probe); the serving listener is bound by tokio directly.
    refuse_if_live(&socket_path)?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build tokio runtime")?;
    let socket_path_for_bind = socket_path.clone();
    let data_root_for_serve = data_root.clone();
    runtime.block_on(async move {
        use std::os::unix::fs::PermissionsExt as _;
        let listener = tokio::net::UnixListener::bind(&socket_path_for_bind)
            .with_context(|| format!("bind {}", socket_path_for_bind.display()))?;
        std::fs::set_permissions(
            &socket_path_for_bind,
            std::fs::Permissions::from_mode(0o600),
        )
        .context("chmod socket 0600")?;
        tracing::info!(
            data_root = %data_root_for_serve.display(),
            socket = %socket_path_for_bind.display(),
            "kallip-daemon listening"
        );
        // The reconcile sweep keeps its snapshot inside its own task; see
        // the module doc for why the daemon proper stays stateless.
        tokio::spawn(reconcile::run(data_root_for_serve.clone()));
        let daemon = server::Daemon::new(data_root_for_serve);
        daemon.serve(listener).await
    })
}

/// Instance tree root: `KALLIP_DAEMON_DATA_DIR` verbatim, else the XDG data
/// home's `kallipai/tagmata` directory (matching
/// `kallip_runtime::persistence`'s default, so daemon and instances agree
/// on where the tree lives without sharing code). Every boot names itself
/// with `KALLIP_TAGMA_SLUG` and owns that slug's leaf; the state side
/// lives under `kallipai/daemon` beside the control socket.
fn data_root() -> Result<PathBuf> {
    // The base lookup stays lazy: an explicit override must keep working
    // where the platform data home cannot be determined (a unit with
    // the env set but no HOME), so only the None branch ever looks it up.
    match std::env::var_os("KALLIP_DAEMON_DATA_DIR") {
        Some(dir) => Ok(resolve_data_root(Some(dir), PathBuf::new())),
        None => Ok(resolve_data_root(
            None,
            dirs::data_dir().context("could not determine platform data directory")?,
        )),
    }
}

/// Pure resolution of the instance-tree root: an explicit override wins
/// verbatim (a set-but-empty value included - it is still a set value),
/// else the platform data home gains exactly the `kallipai/tagmata`
/// segments - the same tree every slug-named boot derives. Pure so the
/// default shape is testable without touching process-environment state.
fn resolve_data_root(override_dir: Option<OsString>, data_home: PathBuf) -> PathBuf {
    match override_dir {
        Some(dir) => PathBuf::from(dir),
        None => data_home.join("kallipai").join("tagmata"),
    }
}

/// Refuse to take over a live daemon's socket (root 16:31Z mandate): a
/// refused-or-absent socket file is stale — unlink so the async bind
/// starts clean; a successful connect means a live listener owns the
/// endpoint and startup must fail instead of stealing it.
fn refuse_if_live(path: &std::path::Path) -> Result<()> {
    match std::os::unix::net::UnixStream::connect(path) {
        Ok(_probe) => {
            tracing::error!(
                socket = %path.display(),
                "another daemon owns this socket; refusing to start"
            );
            anyhow::bail!(
                "another kallip-daemon is listening at {}; refusing to take over a live socket",
                path.display()
            );
        }
        Err(_) => {
            let _ = std::fs::remove_file(path);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Both branches of the pure resolution: an override wins verbatim
    /// (set-but-empty included - it is still a set value), and the default
    /// gains exactly the kallipai/tagmata segments.
    #[test]
    fn resolve_data_root_overrides_win_verbatim() {
        assert_eq!(
            resolve_data_root(Some("/custom/root".into()), PathBuf::from("/xdg/data")),
            PathBuf::from("/custom/root")
        );
        assert_eq!(
            resolve_data_root(Some(String::new().into()), PathBuf::from("/xdg/data")),
            PathBuf::from("")
        );
    }

    /// The default carries exactly the kallipai/tagmata segments - the
    /// same tree every slug-named boot derives - pinning the shape
    /// against a doubled-namespace regression.
    #[test]
    fn resolve_data_root_default_joins_kallipai_tagmata_segments() {
        assert_eq!(
            resolve_data_root(None, PathBuf::from("/xdg/data")),
            PathBuf::from("/xdg/data/kallipai/tagmata")
        );
    }
}

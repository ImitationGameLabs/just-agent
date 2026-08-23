//! kallip-daemon: stateless, directory-driven manager for local kallip
//! instances.
//!
//! Configuration is exactly two environment variables — no config file:
//! - `KALLIP_DATA_DIR`: the instance tree root (default
//!   `~/.local/share/kallip`); each child directory with an `instance.id`
//!   is a managed instance.
//! - `KALLIP_STATE_DIR`: daemon-owned state, the control socket's home
//!   (rootless default `~/.local/state/kallip-daemon`; a system install
//!   points it at `/run/kallip-daemon` via the unit).
//!
//! The control socket is 0600: filesystem permission is the only auth.

mod bins;
mod scan;
mod server;
mod spawn;
mod stop;

use std::path::PathBuf;

use anyhow::{Context as _, Result};

fn main() -> Result<()> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let data_root = data_root()?;
    let state_dir = state_dir()?;
    let socket_path = socket_path(&state_dir);

    // Prove-liveness socket takeover: a blind unlink
    // could steal the endpoint of a live daemon — the first daemon keeps
    // running but loses its socket file. Instead, bind; on EADDRINUSE,
    // probe by connecting: a live listener refuses startup ("already
    // running"), a refused connection means the file is stale, unlink and
    // bind once more.
    std::fs::create_dir_all(&state_dir).context("create state dir")?;
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
        let daemon = server::Daemon::new(data_root_for_serve);
        daemon.serve(listener).await
    })
}

/// Instance tree root: `KALLIP_DATA_DIR` verbatim, else the XDG data home
/// namespaced `kallip` (matching `kallip_runtime::persistence`'s default, so
/// daemon and instances agree on where the tree lives without sharing code).
fn data_root() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("KALLIP_DATA_DIR") {
        return Ok(PathBuf::from(dir));
    }
    dirs_data_home().map(|home| home.join("kallip"))
}

/// Daemon-owned state dir: `KALLIP_STATE_DIR` verbatim, else the XDG state
/// home namespaced `kallip-daemon`.
fn state_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("KALLIP_STATE_DIR") {
        return Ok(PathBuf::from(dir));
    }
    Ok(dirs::state_dir()
        .context("could not determine platform state directory")?
        .join("kallip-daemon"))
}

fn dirs_data_home() -> Result<PathBuf> {
    Ok(dirs::data_dir()
        .context("could not determine platform data directory")?
        .join("kallip"))
}

/// Refuse to take over a live daemon's socket (root 16:31Z mandate): a
/// refused-or-absent socket file is stale — unlink so the async bind
/// starts clean; a successful connect means a live listener owns the
/// endpoint and startup must fail instead of stealing it.
fn refuse_if_live(path: &std::path::Path) -> Result<()> {
    match std::os::unix::net::UnixStream::connect(path) {
        Ok(_probe) => anyhow::bail!(
            "another kallip-daemon is listening at {}; refusing to take over a live socket",
            path.display()
        ),
        Err(_) => {
            let _ = std::fs::remove_file(path);
            Ok(())
        }
    }
}

fn socket_path(state_dir: &std::path::Path) -> PathBuf {
    match std::env::var_os("KALLIP_DAEMON_SOCKET") {
        Some(path) => PathBuf::from(path),
        None => state_dir.join("control.sock"),
    }
}

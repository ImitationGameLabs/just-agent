//! Stop semantics: read the pid from runtime.json → verify it is this
//! instance's own live tagma (launch anchor or name chain; the pid
//! reuse guard) → SIGTERM → poll for exit within the grace period →
//! SIGKILL. runtime.json stays (adoption semantics: a daemon restart
//! rebuilds its view from the tree).

use std::path::Path;
use std::time::{Duration, Instant};

use kallip_daemon_common::wire::ErrorCode;

#[derive(Debug, thiserror::Error)]
pub enum StopError {
    #[error("no instance named {0}")]
    NotFound(String),
    #[error("instance {0} is not running (stale or missing pid)")]
    NotRunning(String),
    #[error("stop of {slug} failed after SIGKILL: {message}")]
    Internal { slug: String, message: String },
}

impl From<&StopError> for ErrorCode {
    fn from(error: &StopError) -> Self {
        match error {
            StopError::NotFound(_) => ErrorCode::NotFound,
            StopError::NotRunning(_) => ErrorCode::NotRunning,
            StopError::Internal { .. } => ErrorCode::Internal,
        }
    }
}

const GRACE: Duration = Duration::from_secs(10);

/// Blocking stop. Identity is judged by `scan::identity_matches` —
/// the anchored pid/starttime when meta.json carries one, the
/// exe/comm name chain otherwise — so a recycled pid is refused.
pub fn stop(data_root: &Path, slug: &str) -> Result<(), StopError> {
    let instance_dir = data_root.join(slug);
    let pid: u32 = crate::scan::read_runtime(&instance_dir)
        .map(|runtime| runtime.pid)
        .ok_or_else(|| StopError::NotRunning(slug.to_string()))?;
    if crate::scan::identity_matches(&instance_dir, pid) != crate::scan::Verdict::Match {
        tracing::warn!(
            slug = %slug,
            pid,
            comm = ?crate::scan::pid_comm(pid),
            exe = ?crate::scan::pid_exe(pid),
            "stop refused: recorded pid does not match this instance"
        );
        // Stale runtime state (crash leftover) or a recycled pid: the
        // instance behind it is gone; report it rather than shooting an innocent process.
        return Err(StopError::NotRunning(slug.to_string()));
    }

    send(pid, libc::SIGTERM).map_err(|m| internal(slug, m))?;
    tracing::info!(slug = %slug, pid, "stopping instance; sent SIGTERM");
    let deadline = Instant::now() + GRACE;
    while Instant::now() < deadline {
        if !alive(pid) {
            tracing::info!(slug = %slug, pid, "instance stopped");
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    tracing::warn!(slug = %slug, pid, "grace period expired; escalating to SIGKILL");
    send(pid, libc::SIGKILL).map_err(|m| internal(slug, m))?;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if !alive(pid) {
            tracing::info!(slug = %slug, pid, "instance stopped after SIGKILL");
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    tracing::error!(slug = %slug, pid, "instance survived SIGKILL");
    Err(internal(slug, format!("pid {pid} survived SIGKILL")))
}

fn internal(slug: &str, message: String) -> StopError {
    StopError::Internal {
        slug: slug.to_string(),
        message,
    }
}

fn send(pid: u32, signal: i32) -> Result<(), String> {
    let rc = unsafe { libc::kill(pid as i32, signal) };
    if rc == 0 {
        Ok(())
    } else {
        Err(format!(
            "kill({pid}, {signal}): {}",
            std::io::Error::last_os_error()
        ))
    }
}

/// A zombie counts as exited for our purposes: its /proc entry lingers
/// until reaped, so existence alone would over-report liveness.
fn alive(pid: u32) -> bool {
    crate::scan::pid_is_alive(pid)
}

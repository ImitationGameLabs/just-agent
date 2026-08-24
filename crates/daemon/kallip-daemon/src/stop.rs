//! Stop semantics: read the pid from runtime.json → verify it is really a
//! tagma (pid reuse guard) → SIGTERM → poll for exit within the grace
//! period → SIGKILL. runtime.json stays (adoption semantics: a daemon
//! restart rebuilds its view from the tree).

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

/// Blocking stop. `pid_is_tagma` is injected so tests can fake the
/// liveness verdict without a real process.
pub fn stop(
    data_root: &Path,
    slug: &str,
    pid_is_tagma: &dyn Fn(u32) -> bool,
) -> Result<(), StopError> {
    let instance_dir = data_root.join(slug);
    let pid: u32 = crate::scan::read_runtime(&instance_dir)
        .map(|runtime| runtime.pid)
        .ok_or_else(|| StopError::NotRunning(slug.to_string()))?;
    if !pid_is_tagma(pid) {
        // Stale runtime state (crash leftover) or a recycled pid: the
        // is gone; report it rather than shooting an innocent process.
        return Err(StopError::NotRunning(slug.to_string()));
    }

    send(pid, libc::SIGTERM).map_err(|m| internal(slug, m))?;
    let deadline = Instant::now() + GRACE;
    while Instant::now() < deadline {
        if !alive(pid) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    send(pid, libc::SIGKILL).map_err(|m| internal(slug, m))?;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if !alive(pid) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
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

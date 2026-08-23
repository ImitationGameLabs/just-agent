//! Stop semantics: read pid → verify it is really a
//! tagma (pid reuse guard) → SIGTERM → poll for exit within the grace
//! period → SIGKILL. The pid/port files stay (adoption semantics: a daemon
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
    let pid: u32 = std::fs::read_to_string(instance_dir.join("pid"))
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .ok_or_else(|| StopError::NotRunning(slug.to_string()))?;
    if !pid_is_tagma(pid) {
        // Stale pid file (crash leftover) or a recycled pid: the instance
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

/// A zombie counts as exited for our purposes (reaped or reparented).
fn alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

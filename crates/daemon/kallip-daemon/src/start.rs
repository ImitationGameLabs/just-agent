//! Start semantics: relaunch a previously spawned instance that is now
//! stopped or dead. This is adoption, not allocation — nothing is created
//! or removed: meta.json carries the workspace and user env captured at
//! spawn time, credentials/ survive untouched for the fresh process to
//! pick up, and a stale runtime.json is simply overwritten by the new
//! incarnation's own self-report.

use std::path::Path;
use std::time::Duration;

use kallip_daemon_common::wire::ErrorCode;

use crate::scan;
use crate::spawn::{SpawnError, launch, validate_user_env};

#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error("no instance named {0}")]
    NotFound(String),
    #[error("{0}")]
    Invalid(String),
    #[error("instance {0} is already running")]
    AlreadyRunning(String),
    #[error("instance did not publish pid/port within {timeout_secs}s")]
    Timeout { timeout_secs: u64 },
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl From<&StartError> for ErrorCode {
    fn from(error: &StartError) -> Self {
        match error {
            // Existing-code reuse over a new wire code: an already-running
            // slug is the same resource-conflict family SlugTaken names for
            // spawn; the message carries the precise state.
            StartError::AlreadyRunning(_) => ErrorCode::SlugTaken,
            StartError::NotFound(_) => ErrorCode::NotFound,
            StartError::Invalid(_) => ErrorCode::InvalidSpawnInput,
            StartError::Timeout { .. } => ErrorCode::SpawnTimeout,
            StartError::Internal(_) => ErrorCode::Internal,
        }
    }
}

impl From<SpawnError> for StartError {
    fn from(error: SpawnError) -> Self {
        match error {
            // launch()'s shared failure shapes carry over verbatim; the
            // spawn-only variants cannot occur through start's path but
            // stay total.
            SpawnError::Invalid(m) => StartError::Invalid(m),
            SpawnError::Timeout { timeout_secs } => StartError::Timeout { timeout_secs },
            SpawnError::Internal(e) => StartError::Internal(e),
            other => StartError::Internal(anyhow::anyhow!("{other}")),
        }
    }
}

/// Blocking relaunch. `pid_is_tagma` is injected so tests can fake the
/// liveness verdict without a real process (mirroring stop).
pub fn start(
    data_root: &Path,
    slug: &str,
    timeout: Duration,
    pid_is_tagma: &dyn Fn(u32) -> bool,
) -> Result<(u32, u16), StartError> {
    if !kallip_daemon_common::wire::valid_slug(slug) {
        return Err(StartError::Invalid(format!(
            "slug {slug:?} does not match [a-z0-9][a-z0-9-]*"
        )));
    }
    let instance_dir = data_root.join(slug);
    let Some(meta) = scan::read_meta(&instance_dir) else {
        return Err(StartError::NotFound(slug.to_string()));
    };
    let Some(workspace) = meta.workspace.filter(|w| !w.is_empty()) else {
        return Err(StartError::Invalid(format!(
            "instance {slug} has no recorded workspace"
        )));
    };
    if let Some(runtime) = scan::read_runtime(&instance_dir)
        && pid_is_tagma(runtime.pid)
    {
        return Err(StartError::AlreadyRunning(slug.to_string()));
    }
    // The persisted copy is re-validated so hand-edited meta cannot smuggle
    // daemon-owned keys into a launch.
    validate_user_env(&meta.env)?;
    match launch(&instance_dir, Path::new(&workspace), &meta.env, timeout) {
        Ok(started) => Ok(started),
        Err(SpawnError::Timeout { timeout_secs }) => Err(StartError::Timeout { timeout_secs }),
        Err(SpawnError::Internal(e)) => Err(StartError::Internal(e)),
        // Unreachable via launch today; kept total so a future variant
        // surfaces as an internal error instead of failing to map.
        Err(other) => Err(StartError::Internal(anyhow::anyhow!("{other}"))),
    }
}

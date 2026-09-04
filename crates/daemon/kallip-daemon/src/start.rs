//! Start semantics: relaunch a previously spawned instance that is now
//! stopped or dead. This is adoption, not allocation — nothing is created
//! or removed: meta.json carries the workspace and user env captured at
//! spawn time, credentials/ survive untouched for the fresh process to
//! pick up, and a stale runtime.json is removed before the relaunch so
//! the launch poll only ever sees the new incarnation's self-report.

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
    #[error(
        "instance did not publish pid/port within {timeout_secs}s; see the \
         instance log files under <instance-dir>/logs/ and system OOM records"
    )]
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

/// Env key holding the one-time relay enrollment code. Consumed once the
/// instance holds stored relay credentials: tagma's Stored boot branch never
/// reads it, and its boot resolution fail-fasts on stored credentials plus
/// code — so replaying it after a completed enrollment bricks every restart.
const CONSUMED_ENROLLMENT_CODE: &str = "KALLIP_TAGMA_RELAY_ENROLLMENT_CODE";

/// Whether the instance already holds stored relay credentials — the exact
/// predicate of tagma's Stored branch (`credentials::load_tagma`: both
/// `tagma.id` and `tagma.token` readable under the entry dir; the daemon's
/// env-sugar relay entry is named "default"). Deliberately read, not
/// `exists`, so an unreadable file counts as absent on both sides.
/// Cross-crate layout mirror — keep in sync with
/// crates/kallip-tagma/src/credentials.rs.
fn stored_credentials_exist(instance_dir: &Path) -> bool {
    let entry = instance_dir.join("credentials").join("default");
    std::fs::read_to_string(entry.join("tagma.id")).is_ok()
        && std::fs::read_to_string(entry.join("tagma.token")).is_ok()
}

/// Build the replay env for a restart, dropping the enrollment code once it
/// is provably spent (stored credentials exist) and scrubbing it from
/// meta.json in the same stroke — single-use secret material must not sit
/// on disk forever. Conditional on the probe: an instance whose first
/// enrollment failed (that boot degrades the entry to local-only, it does
/// not fail) still needs the code on restart to retry, so an unconditional
/// strip would make such instances unbootable. A failed scrub write never
/// blocks the relaunch — the in-memory filter is the functional fix, the
/// persisted copy is hygiene.
fn replay_env(meta: &scan::InstanceMeta, instance_dir: &Path) -> Vec<String> {
    let spent = format!("{CONSUMED_ENROLLMENT_CODE}=");
    let has_code = meta.env.iter().any(|pair| pair.starts_with(&spent));
    if !has_code || !stored_credentials_exist(instance_dir) {
        return meta.env.clone();
    }
    let replay: Vec<String> = meta
        .env
        .iter()
        .filter(|pair| !pair.starts_with(&spent))
        .cloned()
        .collect();
    let scrubbed = scan::InstanceMeta {
        instance_id: meta.instance_id.clone(),
        owner_uid: meta.owner_uid,
        workspace: meta.workspace.clone(),
        env: replay.clone(),
        identity: meta.identity.clone(),
    };
    if let Ok(bytes) = serde_json::to_vec(&scrubbed) {
        let _ = std::fs::write(instance_dir.join("meta.json"), bytes);
    }
    replay
}

/// Blocking relaunch. `pid_is_alive` is injected so tests can fake the
/// liveness verdict without a real process. Liveness
/// alone decides the AlreadyRunning check — the former comm re-check
/// rejected a healthy instance under a wrapped binary name (a
/// makeWrapper-wrapped tagma's truncated comm is not ours to judge).
/// `env_overrides` is a one-shot overlay validated like spawn's request
/// env and applied for this launch only.
pub fn start(
    data_root: &Path,
    slug: &str,
    env_overrides: &[String],
    timeout: Duration,
    pid_is_alive: &dyn Fn(u32) -> bool,
) -> Result<(u32, u16), StartError> {
    if !kallip_daemon_common::wire::valid_slug(slug) {
        return Err(StartError::Invalid(format!(
            "slug {slug:?} does not match [a-z0-9][a-z0-9-]*"
        )));
    }
    // Same rules as spawn's request env, checked before any filesystem
    // work: what could not be sent to spawn cannot overlay a replay.
    validate_user_env(env_overrides)?;
    let instance_dir = data_root.join(slug);
    let Some(meta) = scan::read_meta(&instance_dir) else {
        return Err(StartError::NotFound(slug.to_string()));
    };
    let Some(workspace) = meta.workspace.as_ref().filter(|w| !w.is_empty()) else {
        return Err(StartError::Invalid(format!(
            "instance {slug} has no recorded workspace"
        )));
    };
    if let Some(runtime) = scan::read_runtime(&instance_dir)
        && pid_is_alive(runtime.pid)
    {
        tracing::warn!(
            slug = %slug,
            pid = runtime.pid,
            comm = ?scan::pid_comm(runtime.pid),
            "start refused: recorded pid is alive"
        );
        return Err(StartError::AlreadyRunning(slug.to_string()));
    }
    // The persisted copy is re-validated so hand-edited meta cannot smuggle
    // daemon-owned keys into a launch; the replay env then drops enrollment
    validate_user_env(&meta.env)?;
    // material that is provably spent (see replay_env).
    let replay = replay_env(&meta, &instance_dir);
    // One-shot overlay: appended after the replay so compose_launch_env's
    // map composition lets the later pair win. Not written back — the
    // persisted snapshot stays the spawn-time truth.
    let mut launch_env = replay;
    launch_env.extend(env_overrides.iter().cloned());
    match launch(&instance_dir, Path::new(&workspace), &launch_env, timeout) {
        Ok((pid, port)) => {
            tracing::info!(slug = %slug, pid, port, "instance started (adoption)");
            Ok((pid, port))
        }
        Err(SpawnError::Timeout { timeout_secs }) => Err(StartError::Timeout { timeout_secs }),
        Err(SpawnError::Internal(e)) => Err(StartError::Internal(e)),
        // Unreachable via launch today; kept total so a future variant
        // surfaces as an internal error instead of failing to map.
        Err(other) => Err(StartError::Internal(anyhow::anyhow!("{other}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(env: &[&str]) -> scan::InstanceMeta {
        scan::InstanceMeta {
            instance_id: "instance-1".into(),
            owner_uid: 1000,
            workspace: Some("/ws".into()),
            env: env.iter().map(|pair| pair.to_string()).collect(),
            identity: None,
        }
    }

    fn write_meta(dir: &std::path::Path, value: &scan::InstanceMeta) {
        std::fs::write(
            dir.join("meta.json"),
            serde_json::to_vec(value).expect("serialize meta"),
        )
        .expect("write meta");
    }

    fn persisted_env(dir: &std::path::Path) -> Vec<String> {
        let bytes = std::fs::read(dir.join("meta.json")).expect("read meta");
        let value: scan::InstanceMeta = serde_json::from_slice(&bytes).expect("parse meta");
        value.env
    }

    fn write_stored_credentials(dir: &std::path::Path) {
        let entry = dir.join("credentials").join("default");
        std::fs::create_dir_all(&entry).expect("create entry dir");
        std::fs::write(entry.join("tagma.id"), "tagma-1").expect("write id");
        std::fs::write(entry.join("tagma.token"), "token").expect("write token");
    }

    #[test]
    fn replay_drops_spent_code_and_scrubs_meta() {
        let dir = tempfile::tempdir().expect("instance tempdir");
        write_stored_credentials(dir.path());
        let value = meta(&[
            "KALLIP_OPERATOR_TOKEN=t",
            "KALLIP_TAGMA_RELAY_ENROLLMENT_CODE=sk-spent",
        ]);
        write_meta(dir.path(), &value);

        let replay = replay_env(&value, dir.path());

        assert_eq!(replay, ["KALLIP_OPERATOR_TOKEN=t"]);
        assert_eq!(persisted_env(dir.path()), ["KALLIP_OPERATOR_TOKEN=t"]);
    }

    #[test]
    fn replay_scrub_preserves_the_identity_anchor() {
        // The scrub rewrites meta.json wholesale; dropping the anchor
        // here would silently demote every enrolled instance to
        // name-chain classification after its first code scrub.
        let dir = tempfile::tempdir().expect("instance tempdir");
        write_stored_credentials(dir.path());
        let mut value = meta(&[
            "KALLIP_OPERATOR_TOKEN=t",
            "KALLIP_TAGMA_RELAY_ENROLLMENT_CODE=sk-spent",
        ]);
        value.identity = Some(scan::Identity {
            pid: 7,
            starttime: 99,
            anchored_at: 5,
        });
        write_meta(dir.path(), &value);

        replay_env(&value, dir.path());

        let bytes = std::fs::read(dir.path().join("meta.json")).expect("read meta");
        let parsed: scan::InstanceMeta = serde_json::from_slice(&bytes).expect("parse meta");
        let identity = parsed.identity.expect("anchor survives the scrub");
        assert_eq!((identity.pid, identity.starttime), (7, 99));
    }

    #[test]
    fn replay_keeps_code_while_enrollment_is_incomplete() {
        // No stored credentials: a first enrollment that degraded to
        // local-only still needs the code on restart to retry.
        let dir = tempfile::tempdir().expect("instance tempdir");
        let value = meta(&[
            "KALLIP_OPERATOR_TOKEN=t",
            "KALLIP_TAGMA_RELAY_ENROLLMENT_CODE=sk-fresh",
        ]);
        write_meta(dir.path(), &value);

        let replay = replay_env(&value, dir.path());

        assert_eq!(replay, value.env);
        assert_eq!(persisted_env(dir.path()), value.env);
    }

    #[test]
    fn replay_without_code_is_identity() {
        let dir = tempfile::tempdir().expect("instance tempdir");
        write_stored_credentials(dir.path());
        let value = meta(&["KALLIP_OPERATOR_TOKEN=t"]);
        write_meta(dir.path(), &value);

        assert_eq!(replay_env(&value, dir.path()), ["KALLIP_OPERATOR_TOKEN=t"]);
    }

    #[test]
    fn start_rejects_overlay_key_outside_the_allowlist() {
        // Validation fires before any filesystem work, so the path is
        // never read and no meta.json is needed.
        let error = start(
            std::path::Path::new("."),
            "instance-1",
            &["SOME_OTHER_KEY=v".to_string()],
            Duration::from_secs(1),
            &|_| false,
        )
        .expect_err("non-allowlisted overlay key");
        assert!(error.to_string().contains("allowlisted"), "{error}");
    }

    #[test]
    fn start_rejects_overlay_on_daemon_owned_key() {
        let error = start(
            std::path::Path::new("."),
            "instance-1",
            &["KALLIP_TAGMA_ADDR=127.0.0.1:1".to_string()],
            Duration::from_secs(1),
            &|_| false,
        )
        .expect_err("reserved overlay key");
        assert!(
            error.to_string().contains("cannot be overridden"),
            "{error}"
        );
    }
}

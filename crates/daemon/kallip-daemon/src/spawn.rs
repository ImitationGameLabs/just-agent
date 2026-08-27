//! The spawn pipeline: validate → allocate → (uid provisioning is a
//! no-op in the same-uid profile) → detach-exec via the helper →
//! wait for the instance's self-written runtime.json, rolling back on timeout.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use kallip_daemon_common::wire::valid_slug;

use crate::bins;
use crate::scan;

/// How the request can fail, mapped 1:1 onto wire error codes by the server.
#[derive(Debug, thiserror::Error)]
pub enum SpawnError {
    #[error("slug {0:?} already exists")]
    SlugTaken(String),
    #[error("workspace {requested} overlaps instance {existing_slug} ({existing_workspace})")]
    Overlap {
        requested: String,
        existing_slug: String,
        existing_workspace: String,
    },
    #[error("{0}")]
    Invalid(String),
    #[error("instance did not publish pid/port within {timeout_secs}s")]
    Timeout { timeout_secs: u64 },
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

/// Env keys the daemon owns; a request may not override them.
const RESERVED_KEYS: [&str; 3] = [
    "KALLIP_DATA_DIR",
    "KALLIP_WORKSPACE_ROOT",
    "KALLIP_TAGMA_ADDR",
];

/// Spawn one instance under `data_root`. Blocking — the server runs it on
/// the connection task.
/// The instance is owned by `owner_uid` (the requesting peer).
#[allow(clippy::too_many_arguments)]
pub fn spawn(
    data_root: &Path,
    slug: &str,
    workspace: &str,
    user_env: &[String],
    timeout: Duration,
    owner_uid: u32,
) -> Result<(u32, u16), SpawnError> {
    // --- validate ---------------------------------------------------------
    if !valid_slug(slug) {
        return Err(SpawnError::Invalid(format!(
            "slug {slug:?} does not match [a-z0-9][a-z0-9-]*"
        )));
    }
    let instance_dir = data_root.join(slug);
    if instance_dir.exists() {
        return Err(SpawnError::SlugTaken(slug.to_string()));
    }
    let workspace_path = PathBuf::from(workspace);
    if !workspace_path.is_dir() {
        return Err(SpawnError::Invalid(format!(
            "workspace {workspace:?} is not an existing directory"
        )));
    }
    let workspace_canon = workspace_path
        .canonicalize()
        .map_err(|e| SpawnError::Invalid(format!("canonicalizing workspace: {e}")))?;

    // Workspace disjointness: against every existing instance's workspace
    // and against the instance tree itself (an agent whose workspace is the
    // tree could write another instance's metadata).
    let data_root_canon = data_root
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("canonicalizing data root: {e}"))?;
    if overlaps(&workspace_canon, &data_root_canon) {
        return Err(SpawnError::Overlap {
            requested: workspace.to_string(),
            existing_slug: "(instance tree)".into(),
            existing_workspace: data_root.display().to_string(),
        });
    }
    for instance in scan::scan_instances(data_root) {
        if let Some(existing) = instance.workspace {
            let existing_path = PathBuf::from(&existing);
            if overlaps(&workspace_canon, &existing_path) {
                return Err(SpawnError::Overlap {
                    requested: workspace.to_string(),
                    existing_slug: instance.slug,
                    existing_workspace: existing,
                });
            }
        }
    }

    validate_user_env(user_env)?;

    // --- allocate ---------------------------------------------------------
    std::fs::create_dir(&instance_dir)
        .map_err(|e| anyhow::anyhow!("creating instance dir: {e}"))?;
    let instance_id = uuid::Uuid::new_v4().to_string();
    let rolled_back = |e| {
        // Best-effort rollback: the allocation this call created goes away.
        let _ = std::fs::remove_dir_all(&instance_dir);
        SpawnError::Internal(e)
    };
    let meta_bytes = serde_json::to_vec(&scan::InstanceMeta {
        instance_id,
        owner_uid,
        workspace: Some(workspace_canon.display().to_string()),
        env: user_env.to_vec(),
    })
    .map_err(|e| rolled_back(anyhow::anyhow!("serializing meta.json: {e}")))?;
    std::fs::write(instance_dir.join("meta.json"), &meta_bytes)
        .map_err(|e| rolled_back(anyhow::anyhow!("writing meta.json: {e}")))?;

    // --- detach-exec + adopt ---------------------------------------------
    launch(&instance_dir, &workspace_canon, user_env, timeout).inspect_err(|_| {
        // Rollback: this fresh allocation goes away on any failure — kill
        // whatever the helper left first (a failed exec leaves nothing;
        // a half-boot leaves a running tagma). start() shares launch but
        // keeps an existing tree, so identity and credentials survive a
        // failed relaunch.
        if let Some(pid) = scan::read_runtime(&instance_dir).map(|r| r.pid) {
            unsafe { libc::kill(pid as i32, libc::SIGKILL) };
        }
        let _ = std::fs::remove_dir_all(&instance_dir);
    })
}

/// One path contains the other (ancestor/descendant), including equality.
/// Non-canonical `b` still compares correctly when it is a prefix/suffix
/// match of the canonical `a` only in pathological trees; existing
/// workspaces were canonicalized when written.
fn overlaps(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

/// Request env pairs: KEY=VALUE shape, KALLIP_* (or RUST_LOG), none of
/// the daemon-owned keys. Shared by spawn (fresh request env) and start
/// (re-validating the persisted copy against hand-edited meta files).
pub(crate) fn validate_user_env(user_env: &[String]) -> Result<(), SpawnError> {
    for pair in user_env {
        let Some((key, value)) = pair.split_once('=') else {
            return Err(SpawnError::Invalid(format!(
                "env arg {pair:?} is not KEY=VALUE"
            )));
        };
        if RESERVED_KEYS.contains(&key) {
            return Err(SpawnError::Invalid(format!(
                "env key {key} is set by the daemon and cannot be overridden"
            )));
        }
        if !(key.starts_with("KALLIP_") || key == "RUST_LOG") {
            return Err(SpawnError::Invalid(format!(
                "env key {key:?} is not allowlisted (KALLIP_* or RUST_LOG)"
            )));
        }
        if value.is_empty() {
            return Err(SpawnError::Invalid(format!(
                "env arg {pair:?} has an empty value"
            )));
        }
    }
    Ok(())
}

/// Detach-exec one instance's tagma via the spawn helper and wait until the
/// process publishes its own runtime.json. Shared tail of spawn (fresh
/// tree) and start (adoption of an existing tree); callers re-validate
/// user env before reaching here. On failure the tree is left standing —
/// cleanup policy belongs to the caller (spawn removes its own fresh
/// allocation, start keeps an existing one) — but a half-booted leftover
/// is SIGKILLed here either way so no orphan outlives the timeout.
pub(crate) fn launch(
    instance_dir: &Path,
    workspace_canon: &Path,
    user_env: &[String],
    timeout: Duration,
) -> Result<(u32, u16), SpawnError> {
    let helper = bins::resolve("kallip-daemon-spawn");
    let tagma = bins::resolve("kallip-tagma");
    let mut env: Vec<String> = vec![
        format!("KALLIP_DATA_DIR={}", instance_dir.display()),
        format!("KALLIP_WORKSPACE_ROOT={}", workspace_canon.display()),
        "KALLIP_TAGMA_ADDR=127.0.0.1:0".to_string(),
        "RUST_LOG=info".to_string(),
    ];
    env.extend(user_env.iter().cloned());
    let status = std::process::Command::new(&helper)
        .arg(instance_dir)
        .arg(&tagma)
        .args(&env)
        .status()
        .map_err(|e| anyhow::anyhow!("running spawn helper: {e}"))?;
    if !status.success() {
        return Err(anyhow::anyhow!("spawn helper exited {status}").into());
    }

    // --- wait for the self-written runtime.json --------------------------
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(runtime) = scan::read_runtime(instance_dir)
            && scan::pid_is_tagma(runtime.pid)
        {
            return Ok((runtime.pid, runtime.port));
        }
        if Instant::now() >= deadline {
            // Kill whatever the helper left; keep the tree itself.
            if let Some(pid) = scan::read_runtime(instance_dir).map(|r| r.pid) {
                unsafe { libc::kill(pid as i32, libc::SIGKILL) };
            }
            return Err(SpawnError::Timeout {
                timeout_secs: timeout.as_secs(),
            });
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

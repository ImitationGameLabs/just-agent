//! The spawn pipeline: validate → allocate → (uid provisioning is a
//! no-op in the same-uid profile) → harvest the login environment →
//! detach-exec via the helper → wait for the instance's self-written
//! runtime.json, rolling back on timeout.

use std::collections::BTreeMap;
use std::ffi::{CStr, OsStr, OsString};
use std::io::Read;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::process::CommandExt as _;
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
    #[error(
        "instance did not publish pid/port within {timeout_secs}s; see the \
         instance log files under <instance-dir>/logs/ and system OOM records"
    )]
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

// --- login-environment harvest -------------------------------------------

/// Bash used for the login harvest: an explicit path, never `$SHELL`, so
/// the user's shell preference cannot swap the interpreter under us.
const HARVEST_BASH: &str = "/bin/bash";

/// Wall-clock budget one launch may spend harvesting; past it the shell is
/// killed and the launch degrades to the fallback PATH.
const HARVEST_TIMEOUT: Duration = Duration::from_secs(2);

/// Ceiling on the harvested stream: a profile spamming output fails the
/// harvest instead of wedging the launch.
const HARVEST_MAX_BYTES: usize = 1024 * 1024;

/// Ceiling on a single harvested value: execve caps one argument or
/// environment string at MAX_ARG_STRLEN (128 KiB), and a longer value
/// would fail the helper spawn — the opposite of degrading. Half the
/// cap leaves room for the key and the rest of the launch arguments.
const HARVEST_MAX_VALUE_BYTES: usize = 64 * 1024;

/// The system tail of the fallback PATH used when harvesting fails.
const FALLBACK_PATH_TAIL: &str = ":/usr/local/bin:/usr/bin:/bin";

/// Why a harvest failed. Every variant degrades the launch to the fallback
/// PATH — a broken profile must not cost the instance its boot.
#[derive(Debug, thiserror::Error)]
enum HarvestError {
    #[error("harvest shell could not run: {0}")]
    Spawn(String),
    #[error("harvest shell exited with {0}")]
    Exit(std::process::ExitStatus),
    #[error("harvest shell exceeded the time budget")]
    Timeout,
    #[error("harvested stream exceeded the size cap")]
    TooLarge,
    #[error("harvested environment has no PATH key")]
    NoPath,
}

/// The minimal identity a login shell needs before it can find the user's
/// profile chain: HOME locates ~/.profile; USER/LOGNAME are what profile
/// scripts expect to see.
#[derive(Debug, Default)]
struct HarvestSeed {
    home: Option<OsString>,
    user: Option<OsString>,
}

impl HarvestSeed {
    /// Identity of the user the daemon itself runs as: the daemon's own
    /// environment first, the passwd entry for whatever key is missing.
    ///
    /// Invariant: in the same-uid profile the daemon environment *is* the
    /// executing user's environment, so seeding from it is
    /// self-description, not caller trust. A setuid form must instead
    /// seed (and run the whole harvest) inside the per-owner execution
    /// context — never from the requesting peer's environment.
    fn for_current_process() -> Self {
        let passwd = cached_passwd_identity();
        let home = std::env::var_os("HOME").or_else(|| passwd.as_ref().map(|(_, dir)| dir.clone()));
        let user = std::env::var_os("USER").or_else(|| passwd.map(|(name, _)| name));
        Self { home, user }
    }
}

/// Cached passwd identity of the effective user: getpwuid is not
/// thread-safe (shared NSS buffers) while launches run concurrently on
/// the server's blocking pool, and the daemon's euid never changes
/// during its lifetime — so read it once and share the snapshot.
fn cached_passwd_identity() -> Option<(OsString, OsString)> {
    static PASSWD: std::sync::OnceLock<Option<(OsString, OsString)>> = std::sync::OnceLock::new();
    PASSWD
        .get_or_init(|| {
            // SAFETY: getpwuid returns a libc-owned struct or null; both
            // strings are copied out before any other libc call can
            // touch the buffer, and the OnceLock runs this exactly once
            // so concurrent launches cannot interleave the call.
            let pw = unsafe { libc::getpwuid(libc::geteuid()) };
            let pw = unsafe { pw.as_ref() }?;
            if pw.pw_name.is_null() || pw.pw_dir.is_null() {
                return None;
            }
            // SAFETY: passwd fields are NUL-terminated C strings.
            let name = unsafe { CStr::from_ptr(pw.pw_name) }.to_bytes();
            let dir = unsafe { CStr::from_ptr(pw.pw_dir) }.to_bytes();
            Some((
                OsStr::from_bytes(name).to_owned(),
                OsStr::from_bytes(dir).to_owned(),
            ))
        })
        .clone()
}

/// Run one login shell under a cleared environment (seeding only the
/// identity above) and capture the environment it computes. `env -0`
/// keeps multiline values intact; the NUL-separated stream is parsed
/// strictly — see [`parse_harvest_output`].
fn harvest_login_env(
    bash: &Path,
    seed: &HarvestSeed,
    timeout: Duration,
) -> Result<Vec<(String, String)>, HarvestError> {
    let mut command = std::process::Command::new(bash);
    command
        .args(["-l", "-c", "env -0"])
        // Own process group: a timeout must kill the whole tree — a
        // profile background job inherits the pipe and would otherwise
        // outlive the shell, holding the harvest open with it.
        .process_group(0)
        .env_clear()
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    if let Some(home) = &seed.home {
        command.env("HOME", home);
    }
    if let Some(user) = &seed.user {
        command.env("USER", user).env("LOGNAME", user);
    }
    let mut child = command
        .spawn()
        .map_err(|e| HarvestError::Spawn(e.to_string()))?;

    // Drain stdout on its own thread: a profile writing more than the
    // pipe buffer would otherwise deadlock against the wait loop below.
    // The reader enforces the stream cap while reading — an endless
    // writer must not grow memory waiting for an EOF that never comes.
    let pipe = child.stdout.take().expect("piped stdout");
    let (is_done, reader_done) = std::sync::mpsc::channel::<()>();
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = pipe
            .take(HARVEST_MAX_BYTES as u64 + 1)
            .read_to_end(&mut buf);
        let _ = is_done.send(());
        buf
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    kill_process_group(&mut child);
                    return Err(HarvestError::Timeout);
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(HarvestError::Spawn(e.to_string())),
        }
    };
    if !status.success() {
        kill_process_group(&mut child);
        return Err(HarvestError::Exit(status));
    }
    // The shell is gone, but a profile background job may still hold
    // the pipe: wait for the reader inside the budget, never join
    // blindly — the join would block for the job's whole lifetime.
    let grace = deadline.max(Instant::now() + Duration::from_millis(200));
    if reader_done
        .recv_timeout(grace.saturating_duration_since(Instant::now()))
        .is_err()
    {
        kill_process_group(&mut child);
        return Err(HarvestError::Timeout);
    }
    let output = reader
        .join()
        .map_err(|_| HarvestError::Spawn("stdout reader panicked".into()))?;
    let parsed = parse_harvest_output(&output);
    if parsed.is_err() {
        // A capped stream means a background writer may still be alive
        // past the shell; every failure path leaves a clean process group.
        kill_process_group(&mut child);
    }
    parsed
}

/// Kill the whole harvest process group — the shell plus any profile
/// background job that inherited its stdio — and reap the shell. The
/// group id equals the shell's pid because the command spawns inside
/// `process_group(0)`. Safe to call once per failure path; killing a
/// dead group is a no-op ESRCH.
fn kill_process_group(child: &mut std::process::Child) {
    // SAFETY: kill(2) on a process group we created; errors (ESRCH on
    // an already-dead group) are meaningless here.
    unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
    let _ = child.kill();
    let _ = child.wait();
}

/// Parse the NUL-separated stream `env -0` produced. Each token must be
/// a well-formed KEY=VALUE with a legal key name; anything else — the
/// empty tail, profile stdout glued onto the first token, stray output —
/// is dropped with a warning. Later duplicates win, matching shell
/// `export` semantics. A stream without a PATH key counts as total
/// failure so the caller falls back rather than launching a blank-PATH
/// instance.
fn parse_harvest_output(bytes: &[u8]) -> Result<Vec<(String, String)>, HarvestError> {
    let mut pairs: Vec<(String, String)> = Vec::new();
    if bytes.len() > HARVEST_MAX_BYTES {
        return Err(HarvestError::TooLarge);
    }
    for token in bytes.split(|&b| b == 0) {
        if token.is_empty() {
            continue;
        }
        let token = String::from_utf8_lossy(token);
        let Some((key, value)) = token.split_once('=') else {
            tracing::warn!(%token, "harvest: dropping token without KEY=VALUE shape");
            continue;
        };
        if !valid_env_key(key) {
            tracing::warn!(%key, "harvest: dropping token with a malformed key");
            continue;
        }
        if value.len() > HARVEST_MAX_VALUE_BYTES {
            tracing::warn!(key = %key, "harvest: dropping oversized value");
            continue;
        }
        if key == "PATH" && value.is_empty() {
            // Defensive: an empty PATH is as unusable as a missing one,
            // and the explicit channel rejects empty values the same way.
            tracing::warn!("harvest: empty PATH value treated as missing");
            continue;
        }
        if let Some(slot) = pairs.iter_mut().find(|(k, _)| k == key) {
            slot.1 = value.to_owned();
        } else {
            pairs.push((key.to_owned(), value.to_owned()));
        }
    }
    if pairs.iter().any(|(k, _)| k == "PATH") {
        Ok(pairs)
    } else {
        Err(HarvestError::NoPath)
    }
}

/// `[A-Za-z_][A-Za-z0-9_]*` — the key shape every consumer of these pairs
/// assumes; anything else means pollution, not environment.
fn valid_env_key(key: &str) -> bool {
    let mut chars = key.chars();
    matches!(chars.next(), Some('a'..='z' | 'A'..='Z' | '_'))
        && chars.all(|c| matches!(c, 'a'..='z' | 'A'..='Z' | '0'..='9' | '_'))
}

/// The degraded PATH for a failed harvest: the daemon's own binary
/// directory (where the helper and tagma resolve from) plus the
/// conventional system locations.
fn fallback_path(bin_dir: Option<&Path>) -> String {
    match bin_dir.filter(|dir| !dir.as_os_str().is_empty()) {
        Some(dir) => format!("{}{FALLBACK_PATH_TAIL}", dir.display()),
        None => FALLBACK_PATH_TAIL.trim_start_matches(':').to_owned(),
    }
}

/// The launch env base: the harvested login environment, or — on any
/// harvest failure — the fallback PATH. Always a usable base: harvesting
/// is a best-effort upgrade, never a launch gate.
fn harvest_base_env(fallback_bin_dir: Option<&Path>) -> Vec<(String, String)> {
    let seed = HarvestSeed::for_current_process();
    match harvest_login_env(Path::new(HARVEST_BASH), &seed, HARVEST_TIMEOUT) {
        Ok(pairs) => pairs,
        Err(error) => {
            tracing::warn!(%error, "login environment harvest failed; using fallback PATH");
            degrade_to_fallback(fallback_bin_dir)
        }
    }
}

/// The degraded base for a failed harvest: just the fallback PATH.
/// Extracted so the Err→fallback→compose chain carries a direct test
/// (the surrounding harvest_base_env shells out and cannot).
fn degrade_to_fallback(fallback_bin_dir: Option<&Path>) -> Vec<(String, String)> {
    vec![("PATH".to_owned(), fallback_path(fallback_bin_dir))]
}

/// Compose the instance env from a map, one value per key:
///
/// * the base (harvest, or fallback PATH) supplies every key it has;
/// * explicit request pairs win per key over the base;
/// * the daemon-owned keys win over everything — a polluted profile
///   cannot smuggle them in;
/// * `RUST_LOG=info` appears only when neither base nor explicit pair
///   supplied one (the old build-then-extend order let the default shadow
///   an explicit value through a duplicate key; map composition ends
///   that);
/// * PATH passes through verbatim, wherever it came from.
fn compose_launch_env(
    base: &[(String, String)],
    user_env: &[String],
    instance_dir: &Path,
    workspace_canon: &Path,
) -> Vec<String> {
    let mut env: BTreeMap<&str, String> = BTreeMap::new();
    for (key, value) in base {
        env.insert(key, value.clone());
    }
    for pair in user_env {
        // Shape, emptiness and key ownership are validated before launch
        // (spawn validates the request; start re-validates the persisted
        // copy) — a pair without `=` here would be a bug elsewhere.
        if let Some((key, value)) = pair.split_once('=') {
            env.insert(key, value.to_owned());
        }
    }
    env.insert("KALLIP_DATA_DIR", instance_dir.display().to_string());
    env.insert(
        "KALLIP_WORKSPACE_ROOT",
        workspace_canon.display().to_string(),
    );
    env.insert("KALLIP_TAGMA_ADDR", "127.0.0.1:0".to_owned());
    env.entry("RUST_LOG").or_insert_with(|| "info".to_owned());
    env.into_iter().map(|(k, v)| format!("{k}={v}")).collect()
}

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
        identity: None,
    })
    .map_err(|e| rolled_back(anyhow::anyhow!("serializing meta.json: {e}")))?;
    std::fs::write(instance_dir.join("meta.json"), &meta_bytes)
        .map_err(|e| rolled_back(anyhow::anyhow!("writing meta.json: {e}")))?;

    // --- detach-exec + adopt ---------------------------------------------
    let started = launch(&instance_dir, &workspace_canon, user_env, timeout).inspect_err(|_| {
        // Rollback: this fresh allocation goes away on any failure — kill
        // whatever the helper left first (a failed exec leaves nothing;
        // a half-boot leaves a running tagma). start() shares launch but
        // keeps an existing tree, so identity and credentials survive a
        // failed relaunch.
        if let Some(pid) = scan::read_runtime(&instance_dir).map(|r| r.pid) {
            tracing::warn!(pid, "spawn rollback: killing half-booted instance");
            unsafe { libc::kill(pid as i32, libc::SIGKILL) };
        }
        if let Err(error) = std::fs::remove_dir_all(&instance_dir) {
            tracing::warn!(%error, "spawn rollback: removing instance dir failed");
        }
    });
    if let Ok((pid, port)) = started {
        tracing::info!(slug = %slug, pid, port, "instance spawned and adopted");
        return Ok((pid, port));
    }
    started
}

/// One path contains the other (ancestor/descendant), including equality.
/// Non-canonical `b` still compares correctly when it is a prefix/suffix
/// match of the canonical `a` only in pathological trees; existing
/// workspaces were canonicalized when written.
fn overlaps(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

/// Request env pairs: KEY=VALUE shape, KALLIP_* (plus RUST_LOG and PATH —
/// both normally arrive via the login harvest; an explicit pair wins),
/// none of the daemon-owned keys. Shared by spawn (fresh request
/// env) and start (re-validating the persisted copy against hand-edited
/// meta files).
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
        if !(key.starts_with("KALLIP_") || key == "RUST_LOG" || key == "PATH") {
            return Err(SpawnError::Invalid(format!(
                "env key {key:?} is not allowlisted (KALLIP_*, RUST_LOG, or PATH)"
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
    // The poll below trusts any runtime.json it sees as belonging to this
    // launch. That trust needs a clean slate: drop a previous
    // incarnation's runtime.json before starting the helper. ENOENT is
    // the common path (fresh spawn); any other failure aborts the launch
    // — an unremovable leftover would leave a foreign pid inside the
    // poll's trust window, and the timeout branch would kill it.
    if let Err(e) = clear_stale_runtime(instance_dir) {
        tracing::error!(
            instance_dir = %instance_dir.display(),
            error = %e,
            "cannot clear stale runtime.json; refusing to launch"
        );
        return Err(anyhow::anyhow!(
            "clearing stale runtime.json in {}: {e}",
            instance_dir.display()
        )
        .into());
    }
    let helper = bins::resolve("kallip-daemon-spawn");
    let tagma = bins::resolve("kallip-tagma");
    let base = harvest_base_env(tagma.parent());
    let env = compose_launch_env(&base, user_env, instance_dir, workspace_canon);
    let status = std::process::Command::new(&helper)
        .arg(instance_dir)
        .arg(&tagma)
        .args(&env)
        .status()
        .map_err(|e| anyhow::anyhow!("running spawn helper: {e}"))?;
    if !status.success() {
        tracing::error!(status = %status, "spawn helper failed");
        return Err(anyhow::anyhow!("spawn helper exited {status}").into());
    }

    // --- wait for the self-written runtime.json --------------------------
    // Invariant: reaching this poll ⇔ the instance dir held no leftover
    // runtime.json at launch time (cleared before the helper ran). Any
    // file that appears during the poll belongs to this launch, so the
    // pid it carries is trusted directly. Trust is then made durable:
    // the claim point pins pid+starttime into meta.json (or clears a
    // stale anchor a previous incarnation left), and a failed
    // revalidation keeps polling — a pid that died between its
    // starttime read and the check must not be reported as launched.
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(runtime) = scan::read_runtime(instance_dir)
            && scan::pid_is_alive(runtime.pid)
            && anchor_identity(instance_dir, runtime.pid)
        {
            return Ok((runtime.pid, runtime.port));
        }
        if Instant::now() >= deadline {
            // Kill whatever the helper left; keep the tree itself.
            if let Some(pid) = scan::read_runtime(instance_dir).map(|r| r.pid) {
                // Diagnosability before the kill: the recorded comm is what
                // a naming-mismatch investigation needs.
                tracing::warn!(
                    pid,
                    comm = ?scan::pid_comm(pid),
                    "launch timed out; killing the pid that never published"
                );
                unsafe { libc::kill(pid as i32, libc::SIGKILL) };
            }
            return Err(SpawnError::Timeout {
                timeout_secs: timeout.as_secs(),
            });
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The launch claim point: pin `pid` into meta.json together with its
/// kernel start time, so later classification can tell this exact
/// incarnation from a reused pid. Post-condition the poll relies on:
/// the anchor names this pid or there is no anchor at all — a stale
/// anchor from a previous incarnation is cleared, never left lying
/// against a pid it does not name. If the anchor write itself fails
/// that is beyond reach: a stale anchor may survive and classify the
/// live pid conservatively (Mismatch); the warn names it. Returns
/// false only when the revalidation race says the pid died under us;
/// the caller keeps polling rather than reporting a launch it cannot
/// vouch for.
fn anchor_identity(instance_dir: &Path, pid: u32) -> bool {
    let starttime = scan::proc_starttime(pid).filter(|t| *t > 0);
    let mut meta = match scan::read_meta(instance_dir) {
        Some(meta) => meta,
        None => {
            tracing::warn!(pid, "meta.json unreadable at claim; launching unanchored");
            return true;
        }
    };
    match starttime {
        Some(starttime) => {
            meta.identity = Some(scan::Identity {
                pid,
                starttime,
                anchored_at: now_unix(),
            });
        }
        None => {
            // Without a starttime there is nothing to pin; make that
            // explicit by clearing any anchor a previous incarnation
            // left, so classification falls to the name chain.
            if meta.identity.is_some() {
                tracing::warn!(pid, "cannot read start time; clearing stale anchor");
            }
            meta.identity = None;
        }
    }
    let bytes = match serde_json::to_vec(&meta) {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::warn!(pid, error = %e, "cannot serialize anchor; stale anchor may remain");
            return true;
        }
    };
    if let Err(e) = std::fs::write(instance_dir.join("meta.json"), bytes) {
        tracing::warn!(pid, error = %e, "cannot write identity anchor; stale anchor may remain");
        return true;
    }
    // Revalidate against what the tree now says: if the pid died
    // between the starttime read and this check, its incarnation is
    // gone and the launch must not claim it.
    match scan::identity_matches(instance_dir, pid) {
        scan::Verdict::Match => true,
        other => {
            tracing::warn!(pid, verdict = ?other, "anchor failed revalidation; not claiming the pid");
            false
        }
    }
}
/// Seconds since the Unix epoch, saturating at 0 on clock skew;
/// diagnostic stamp only.
fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Drop a leftover `runtime.json` from a previous incarnation. ENOENT is
/// success (the target state — no leftover — already holds); any other
/// error surfaces to the caller, which aborts the launch.
fn clear_stale_runtime(instance_dir: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(instance_dir.join("runtime.json")) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- clear_stale_runtime ----------------------------------------------

    #[test]
    fn clear_stale_runtime_is_ok_when_no_file_exists() {
        let dir = tempdir();
        clear_stale_runtime(dir.path()).expect("absent file is the clean state");
    }

    #[test]
    fn clear_stale_runtime_removes_a_leftover() {
        let dir = tempdir();
        std::fs::write(dir.path().join("runtime.json"), b"{}").expect("write leftover");
        clear_stale_runtime(dir.path()).expect("leftover removes");
        assert!(!dir.path().join("runtime.json").exists());
    }

    #[test]
    fn clear_stale_runtime_fails_when_instance_dir_is_not_a_directory() {
        let dir = tempdir();
        let not_a_dir = dir.path().join("file");
        std::fs::write(&not_a_dir, b"x").expect("write file");
        assert!(clear_stale_runtime(&not_a_dir).is_err());
    }

    /// Write an executable `#!/bin/bash` script and return its path. The
    /// body must stick to bash builtins: the shim runs under the
    /// harvest's cleared env, where no PATH exists to resolve external
    /// binaries (the first draft's `sleep 30` failed with exit 127).
    fn shim(dir: &Path, body: &str) -> PathBuf {
        use std::io::Write as _;
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("shim");
        let mut script = std::fs::File::create(&path).expect("create shim");
        writeln!(script, "#!/bin/bash").expect("write shebang");
        writeln!(script, "{body}").expect("write body");
        drop(script);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod shim");
        path
    }

    fn tempdir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    /// The login bash the harvest hardcodes; `None` skips the tests that
    /// need a real shell. Probed by exec, not by stat: under a landlock
    /// sandbox the interpreter is executable while being unstattable.
    fn bash() -> Option<PathBuf> {
        std::process::Command::new("/bin/bash")
            .arg("-c")
            .arg("true")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
            .then(|| PathBuf::from("/bin/bash"))
    }

    // --- parse_harvest_output ------------------------------------------

    #[test]
    fn parse_takes_a_clean_nul_stream() {
        let pairs = parse_harvest_output(b"PATH=/bin:/usr/bin\0HOME=/home/u\0")
            .expect("clean stream parses");
        assert_eq!(pairs[0], ("PATH".to_owned(), "/bin:/usr/bin".to_owned()));
        assert_eq!(pairs[1], ("HOME".to_owned(), "/home/u".to_owned()));
    }

    #[test]
    fn parse_drops_pollution_glued_onto_the_path_token() {
        // Profile stdout lands before `env -0` output with no separator:
        // the PATH token loses its key shape, and a lost PATH must count
        // as total failure (the caller falls back) — never a partial env.
        let bytes = b"banner text\nPATH=/bin\0HOME=/home/u\0";
        assert!(matches!(
            parse_harvest_output(bytes),
            Err(HarvestError::NoPath)
        ));
    }

    #[test]
    fn parse_drops_stray_tokens_but_keeps_valid_ones() {
        let bytes = b"PATH=/bin\0not a pair\0HOME=/home/u\0trailing junk";
        let pairs = parse_harvest_output(bytes).expect("valid keys survive");
        let keys: Vec<&str> = pairs.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["PATH", "HOME"]);
    }

    #[test]
    fn parse_rejects_malformed_key_shapes() {
        for bad in [
            "=v\0PATH=/bin\0",
            "1KEY=v\0PATH=/bin\0",
            "KEY.B=v\0PATH=/bin\0",
        ] {
            let pairs = parse_harvest_output(bad.as_bytes()).expect("PATH present");
            let keys: Vec<&str> = pairs.iter().map(|(k, _)| k.as_str()).collect();
            assert_eq!(keys, ["PATH"], "only well-formed keys survive");
        }
    }

    #[test]
    fn parse_last_duplicate_wins() {
        let pairs = parse_harvest_output(b"PATH=/one\0PATH=/two\0").expect("parses");
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].1, "/two");
    }

    #[test]
    fn parse_empty_stream_is_no_path() {
        assert!(matches!(
            parse_harvest_output(b""),
            Err(HarvestError::NoPath)
        ));
    }

    // --- valid_env_key -------------------------------------------------

    #[test]
    fn env_key_shape() {
        for good in ["PATH", "KALLIP_X", "_x", "a1_"] {
            assert!(valid_env_key(good), "{good}");
        }
        for bad in ["", "1A", "A-B", "A.B", "A B"] {
            assert!(!valid_env_key(bad), "{bad}");
        }
    }

    // --- harvest_login_env (subprocess) -------------------------------

    #[test]
    fn harvest_reads_the_seeded_profile_chain() {
        let Some(bash) = bash() else {
            eprintln!("skip: no /bin/bash on this host");
            return;
        };
        let home = tempdir();
        std::fs::write(
            home.path().join(".profile"),
            "export KALLIP_UNIT_MARKER=green\nexport PATH=/fixture/bin:$PATH\n",
        )
        .expect("write profile");
        let seed = HarvestSeed {
            home: Some(home.path().as_os_str().to_owned()),
            user: Some("probe".into()),
        };
        let pairs = harvest_login_env(&bash, &seed, HARVEST_TIMEOUT).expect("harvest ok");
        let marker = pairs.iter().find(|(k, _)| k == "KALLIP_UNIT_MARKER");
        assert_eq!(marker.map(|(_, v)| v.as_str()), Some("green"));
        let path = pairs.iter().find(|(k, _)| k == "PATH").expect("PATH");
        assert!(
            path.1.starts_with("/fixture/bin"),
            "fixture PATH applied: {}",
            path.1
        );
    }

    #[test]
    fn harvest_shell_failure_maps_to_exit_error() {
        let Some(_) = bash() else {
            eprintln!("skip: no /bin/bash on this host");
            return;
        };
        let dir = tempdir();
        let bash = shim(dir.path(), "exit 3");
        let error = harvest_login_env(&bash, &HarvestSeed::default(), HARVEST_TIMEOUT).unwrap_err();
        assert!(matches!(error, HarvestError::Exit(_)));
    }

    #[test]
    fn harvest_timeout_kills_the_shell_and_returns_promptly() {
        let Some(_) = bash() else {
            eprintln!("skip: no /bin/bash on this host");
            return;
        };
        let dir = tempdir();
        let bash = shim(dir.path(), "while :; do :; done");
        let start = Instant::now();
        let error = harvest_login_env(&bash, &HarvestSeed::default(), Duration::from_millis(300))
            .unwrap_err();
        assert!(matches!(error, HarvestError::Timeout));
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "returned promptly"
        );
    }

    #[test]
    fn harvest_missing_interpreter_maps_to_spawn_error() {
        let error = harvest_login_env(
            Path::new("/nonexistent/bash"),
            &HarvestSeed::default(),
            HARVEST_TIMEOUT,
        )
        .unwrap_err();
        assert!(matches!(error, HarvestError::Spawn(_)));
    }

    // --- compose_launch_env + fallback --------------------------------

    /// A base with every interesting collision in it.
    fn base_env() -> Vec<(String, String)> {
        vec![
            ("PATH".into(), "/harvested/path".into()),
            ("HOME".into(), "/home/harvest".into()),
            ("RUST_LOG".into(), "harvest-level".into()),
            ("KALLIP_DATA_DIR".into(), "/pwned".into()),
            ("EDITOR".into(), "vi".into()),
        ]
    }

    fn get<'a>(env: &'a [String], key: &str) -> Option<&'a str> {
        env.iter()
            .find_map(|pair| pair.strip_prefix(&format!("{key}=")))
    }

    #[test]
    fn compose_explicit_pairs_win_per_key() {
        let env = compose_launch_env(
            &base_env(),
            &["PATH=/explicit".into(), "KALLIP_X=1".into()],
            Path::new("/data/i1"),
            Path::new("/ws"),
        );
        assert_eq!(get(&env, "PATH"), Some("/explicit"));
        assert_eq!(get(&env, "KALLIP_X"), Some("1"));
        assert_eq!(get(&env, "HOME"), Some("/home/harvest"), "harvest key kept");
        assert_eq!(get(&env, "EDITOR"), Some("vi"), "harvest key kept");
    }

    #[test]
    fn compose_daemon_keys_win_over_a_polluted_base() {
        let env = compose_launch_env(&base_env(), &[], Path::new("/data/i1"), Path::new("/ws"));
        assert_eq!(get(&env, "KALLIP_DATA_DIR"), Some("/data/i1"));
        assert_eq!(get(&env, "KALLIP_WORKSPACE_ROOT"), Some("/ws"));
        assert_eq!(get(&env, "KALLIP_TAGMA_ADDR"), Some("127.0.0.1:0"));
    }

    #[test]
    fn compose_rust_log_default_appears_only_when_absent() {
        let explicit = compose_launch_env(
            &base_env(),
            &["RUST_LOG=debug".into()],
            Path::new("/d"),
            Path::new("/w"),
        );
        assert!(explicit.contains(&"RUST_LOG=debug".to_owned()));
        let harvested = compose_launch_env(&base_env(), &[], Path::new("/d"), Path::new("/w"));
        assert!(harvested.contains(&"RUST_LOG=harvest-level".to_owned()));
        let base: Vec<(String, String)> = vec![("PATH".into(), "/p".into())];
        let neither = compose_launch_env(&base, &[], Path::new("/d"), Path::new("/w"));
        assert!(neither.contains(&"RUST_LOG=info".to_owned()));
    }

    #[test]
    fn compose_emits_each_key_exactly_once() {
        let env = compose_launch_env(
            &base_env(),
            &["PATH=/explicit".into(), "RUST_LOG=debug".into()],
            Path::new("/d"),
            Path::new("/w"),
        );
        let mut keys: Vec<&str> = env
            .iter()
            .map(|pair| pair.split('=').next().unwrap())
            .collect();
        let total = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), total, "duplicate keys in {env:?}");
    }

    #[test]
    fn fallback_path_anchors_at_the_binary_directory() {
        assert_eq!(
            fallback_path(Some(Path::new("/opt/kallip/bin"))),
            "/opt/kallip/bin:/usr/local/bin:/usr/bin:/bin"
        );
        assert_eq!(
            fallback_path(Some(Path::new(""))),
            "/usr/local/bin:/usr/bin:/bin"
        );
        assert_eq!(fallback_path(None), "/usr/local/bin:/usr/bin:/bin");
    }

    #[test]
    fn a_failed_harvest_degrades_to_a_usable_base() {
        // The Err branch of harvest_base_env, tested directly: the
        // degraded base flows through compose into a launchable env —
        // if the Err branch ever returned an empty base, nothing else
        // would catch it (the shell-out wrapper cannot be unit-tested).
        let base = degrade_to_fallback(Some(Path::new("/opt/kallip/bin")));
        let env = compose_launch_env(&base, &[], Path::new("/data/i1"), Path::new("/ws"));
        assert!(
            env.contains(&"PATH=/opt/kallip/bin:/usr/local/bin:/usr/bin:/bin".to_owned()),
            "fallback PATH rides through composition: {env:?}"
        );
        assert!(
            env.contains(&"RUST_LOG=info".to_owned()),
            "default fills in"
        );
        assert!(env.contains(&"KALLIP_DATA_DIR=/data/i1".to_owned()));
    }

    #[test]
    fn parse_rejects_a_stream_over_the_total_cap() {
        let mut bytes = b"PATH=/bin\0PAD=".to_vec();
        bytes.extend(std::iter::repeat_n(b'x', HARVEST_MAX_BYTES));
        bytes.push(0);
        assert!(matches!(
            parse_harvest_output(&bytes),
            Err(HarvestError::TooLarge)
        ));
    }

    // --- validate_user_env ---------------------------------------------

    #[test]
    fn validate_accepts_an_explicit_path_key() {
        validate_user_env(&["PATH=/custom/bin".into()]).expect("PATH is allowlisted");
        assert!(
            validate_user_env(&["PATH=".into()]).is_err(),
            "empty rejected"
        );
        assert!(
            validate_user_env(&["NOT_KALLIP=1".into()]).is_err(),
            "others rejected"
        );
        assert!(
            validate_user_env(&["KALLIP_DATA_DIR=/x".into()]).is_err(),
            "reserved rejected"
        );
    }

    #[test]
    fn parse_drops_an_oversized_value() {
        // A profile echoing megabytes of KEY=VALUE-shaped text after the
        // real environment stays under the stream cap yet produces one
        // token big enough to fail execve (MAX_ARG_STRLEN) — the harvest
        // must drop it, not hand it to the launch args.
        let mut bytes = b"PATH=/bin\0BIG=".to_vec();
        bytes.extend(std::iter::repeat_n(b'x', 65 * 1024));
        bytes.push(0);
        let pairs = parse_harvest_output(&bytes).expect("PATH survives");
        let keys: Vec<&str> = pairs.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["PATH"], "only in-cap values survive");
    }

    #[test]
    fn parse_treats_an_empty_path_as_missing() {
        // Defensive depth: an empty PATH is as unusable as none, and the
        // explicit channel rejects empty values the same way.
        assert!(matches!(
            parse_harvest_output(b"PATH=\0HOME=/home/u\0"),
            Err(HarvestError::NoPath)
        ));
    }

    #[test]
    fn harvest_background_pipe_holder_is_bounded() {
        // The wedge shape from review: a profile background job holds
        // the stdout pipe after the shell itself exits. The wait for the
        // reader must stay inside the budget, not inside the job's
        // lifetime (10s here — an unbounded join blocks exactly that
        // long and this test goes red on the elapsed bound).
        let Some(_) = bash() else {
            eprintln!("skip: no /bin/bash on this host");
            return;
        };
        let dir = tempdir();
        let wedge = shim(dir.path(), "read -t 10 x < /dev/zero & exit 0");
        let start = Instant::now();
        let error = harvest_login_env(&wedge, &HarvestSeed::default(), Duration::from_millis(300))
            .unwrap_err();
        assert!(matches!(error, HarvestError::Timeout), "got {error:?}");
        assert!(start.elapsed() < Duration::from_secs(5), "bounded wait");
    }

    #[test]
    fn harvest_stream_cap_is_enforced_while_reading() {
        // Endless writer as a background job: memory must stop at the
        // stream cap (read side) and the result must be a capped error,
        // never an unbounded read or an unbounded wait.
        let Some(_) = bash() else {
            eprintln!("skip: no /bin/bash on this host");
            return;
        };
        let dir = tempdir();
        let spammer = shim(
            dir.path(),
            "while :; do printf 'A%.0s' {1..4096}; done & exit 0",
        );
        let start = Instant::now();
        let error = harvest_login_env(&spammer, &HarvestSeed::default(), Duration::from_secs(2))
            .unwrap_err();
        assert!(
            matches!(error, HarvestError::TooLarge | HarvestError::Timeout),
            "got {error:?}"
        );
        assert!(
            start.elapsed() < Duration::from_secs(6),
            "bounded either way"
        );
    }
}

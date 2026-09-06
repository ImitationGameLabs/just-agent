//! Directory scan: the instance tree IS the registry.
//!
//! The daemon holds no state of its own — `list`/`health` read the tree under
//! the data root fresh on every call, so a daemon restart (or a crash)
//! rebuilds the full view from disk, and manually created directories
//! are managed as long as they carry a daemon-written `meta.json`.
//!
//! Liveness verification has two roots: the launch anchor the daemon's
//! own spawn wrote into `meta.json` (state `running`), and the
//! self-report the live tagma writes into `runtime.json` (pid, kernel
//! starttime, exe family — state `adopted`), which is how a manually
//! launched instance is recognized without a daemon spawn.

use std::path::Path;

use kallip_daemon_common::wire::{HealthReport, InstanceInfo, InstanceState};

/// One scanned instance directory: what the tree says, without judging it.
#[derive(Debug, Clone)]
pub struct ScannedInstance {
    pub slug: String,
    pub instance_id: String,
    pub workspace: Option<String>,
    /// The instance's `runtime.json` pid when present and parse-able.
    pub pid: Option<u32>,
    /// The instance's `runtime.json` listen port. Surfaced so panel
    /// process actions (open) survive a page reload — the session-held
    /// spawn memory is the fallback, not the source.
    pub port: Option<u16>,
    /// The kernel start time the live tagma self-reported in
    /// `runtime.json` (`0`/absent → `None`): the adoption credential
    /// checked when no launch anchor verifies the pid.
    pub reported_starttime: Option<u64>,
    /// The spawn-time uid of the requesting peer, from `meta.json`.
    pub owner: Option<u32>,
    /// The enrolled tagma identity (archeion-issued id) if the instance's own
    /// credentials tree carries one; see `read_tagma_id`.
    pub tagma_id: Option<String>,
    /// The launch-time identity anchor from `meta.json` (the
    /// `identity` key): the kernel incarnation this instance was
    /// claimed as. `None` when the claim point could not pin one —
    /// classification then falls back to the exe/comm name chain.
    pub anchored: Option<Identity>,
}

impl ScannedInstance {
    /// The daemon's classification: a pid verifiably this instance's
    /// live tagma is Running (launch-anchor verified) or Adopted
    /// (runtime.json self-report verified), a missing pid file is a
    /// clean Stopped, and anything else (dead pid, reused pid,
    /// unidentifiable process) is Dead. Single classification source —
    /// info() and health() derive from it.
    pub fn state(&self) -> InstanceState {
        match self.pid {
            None => InstanceState::Stopped,
            Some(pid) => {
                match classify_with_facts(
                    self.anchored.as_ref(),
                    self.reported_starttime,
                    pid,
                    &self.slug,
                ) {
                    (Verdict::Match, Some(Verify::Adopted)) => InstanceState::Adopted,
                    (Verdict::Match, _) => InstanceState::Running,
                    _ => InstanceState::Dead,
                }
            }
        }
    }
    pub fn info(&self) -> InstanceInfo {
        let state = self.state();
        InstanceInfo {
            slug: self.slug.clone(),
            instance_id: self.instance_id.clone(),
            workspace: self.workspace.clone().unwrap_or_default(),
            running: matches!(state, InstanceState::Running | InstanceState::Adopted),
            state,
            // The runtime file survives a stop, so its port is only a
            // live listen port while the instance is live (Running or
            // Adopted); anything else would leak a stale, dead endpoint.
            port: matches!(state, InstanceState::Running | InstanceState::Adopted)
                .then_some(self.port)
                .flatten(),
            owner: self.owner,
            tagma_id: self.tagma_id.clone(),
        }
    }

    pub fn health(&self) -> HealthReport {
        let state = self.state();
        let live = matches!(state, InstanceState::Running | InstanceState::Adopted);
        let detail = if live {
            None
        } else if let Some(pid) = self.pid {
            let (verdict, _) = classify_with_facts(
                self.anchored.as_ref(),
                self.reported_starttime,
                pid,
                &self.slug,
            );
            match verdict {
                Verdict::Unknown if self.reported_starttime.is_some() => Some(
                    "live pid, but its exe is unreadable from the daemon \
                     (cross-uid?); adoption refused"
                        .to_string(),
                ),
                _ => Some("pid does not match a live instance (stale or reused)".to_string()),
            }
        } else {
            Some("no runtime.json".to_string())
        };
        HealthReport {
            slug: Some(self.slug.clone()),
            running: live,
            state,
            detail,
        }
    }
}

/// A live /proc entry is not enough: a zombie keeps its entry (and its
/// comm) until someone reaps it, and under a shell-PID1 init nobody
/// does. The kernel's state field tells a corpse from a live process.
pub fn pid_is_alive(pid: u32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/status")) {
        Ok(status) => !status
            .lines()
            .any(|l| l.starts_with("State:") && l.split_whitespace().nth(1) == Some("Z")),
        Err(_) => false,
    }
}
/// The process name from `/proc/<pid>/comm`, whitespace-trimmed. `None`
/// when the entry is unreadable (the process is gone). This is the
/// diagnostic surface for launch-timeout logs: recording the actual
/// comm value is what makes a naming mismatch investigable.
pub fn pid_comm(pid: u32) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .map(|c| c.trim().to_owned())
}
/// The launch-time identity anchor persisted in `meta.json`: the pid
/// plus the kernel start time of the exact process incarnation a
/// launch claimed. `starttime` is the reuse discriminator (a recycled
/// pid gets a fresh start time); `anchored_at` is a wall-clock
/// diagnostic of when the claim happened, never compared.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Identity {
    pub pid: u32,
    pub starttime: u64,
    #[serde(default)]
    pub anchored_at: u64,
}
/// The kernel start time from `/proc/<pid>/stat` (field 22, clock
/// ticks since boot): the incarnation discriminator that survives pid
/// reuse. Parsed after the comm's closing paren because comm may
/// contain spaces, digits, and parentheses of its own.
pub fn proc_starttime(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_comm = stat.rfind(')')? + 1;
    stat[after_comm..].split_whitespace().nth(19)?.parse().ok()
}
/// The resolved executable path (`/proc/<pid>/exe`). `None` when the
/// link cannot be read — the process is gone, or the reader lacks
/// permission (same-uid always has it).
pub fn pid_exe(pid: u32) -> Option<String> {
    std::fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}
/// Family check on an exe path: the binary's own name or its parent
/// directory names a kallip-tagma once leading dots (the nix wrapper
/// convention `.kallip-tagma-wrapped`) are trimmed. The store layout
/// `<hash>-kallip-tagma-<ver>/bin/kallip-tagma` matches on the binary
/// name, and a binary rebuilt underneath a running process keeps
/// matching through the ` (deleted)` suffix.
fn tagma_exe_family(exe: &str) -> bool {
    let path = std::path::Path::new(exe);
    let file = path.file_name().and_then(|n| n.to_str());
    let parent = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|n| n.to_str());
    [file, parent]
        .into_iter()
        .flatten()
        .any(|n| n.trim_start_matches('.').starts_with("kallip-tagma"))
}
/// The comm fallback: the existing prefix rule plus the nix wrapper
/// shim's exact truncated name (comm caps at 15 bytes).
fn tagma_comm_matches(comm: &str) -> bool {
    comm.trim_start_matches("kallip-").starts_with("tagma") || comm == ".kallip-tagma-w"
}
/// The identity verdict for a recorded pid: is this live process
/// this instance's tagma — the anchored incarnation, or a process
/// the runtime.json self-report vouches for?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Live and positively identified as this instance's tagma.
    Match,
    /// Live, but provably not a verifiable incarnation of this
    /// instance: the anchor denies it, the pid was reused, a foreign
    /// process occupies it, or the self-report describes someone else.
    Mismatch,
    /// Live, but every identity probe failed — unverifiable.
    Unknown,
    /// No live process (dead or zombie).
    Gone,
}
/// How a `Verdict::Match` verified. The wire word hangs off this: an
/// anchor verify is the daemon's own launch claim (`running`), a
/// self-report verify is adoption of a live process the daemon did
/// not launch (`adopted`), and a name verify is the degraded chain
/// worth a warn (still `running`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verify {
    Anchored,
    Adopted,
    ByName,
}
/// What /proc says about a pid, gathered once for classification.
#[derive(Debug, Default, Clone)]
pub struct ProcFacts {
    pub starttime: Option<u64>,
    pub exe: Option<String>,
    pub comm: Option<String>,
}
/// Read the identity facts for `pid` in one pass.
pub fn observe_identity(pid: u32) -> ProcFacts {
    ProcFacts {
        starttime: proc_starttime(pid),
        exe: pid_exe(pid),
        comm: pid_comm(pid),
    }
}
/// Pure classification over the anchored identity, the runtime.json
/// self-report, and the observed facts. The `Verify` is `Some` only
/// on a Match: which root verified — the launch anchor (exact
/// incarnation), the self-report (adoption), or the exe/comm name
/// chain (degraded). Level order: existence (a dead pid is quietly
/// `Gone`), a positive anchor verify, the anti split-brain anchor
/// denial, the adoption triple (the self-reported starttime matches
/// the live process AND the exe is family — a readable exe is also
/// the same-uid proof, so a cross-uid tagma is refused as `Unknown`
/// rather than adopted), then the legacy name chain for runtimes
/// without a starttime credential.
fn classify_identity(
    anchored: Option<&Identity>,
    reported_starttime: Option<u64>,
    pid: u32,
    facts: &ProcFacts,
) -> (Verdict, Option<Verify>) {
    if !pid_is_alive(pid) {
        return (Verdict::Gone, None);
    }
    if let Some(anchor) = anchored {
        if anchor.pid == pid && anchor.starttime != 0 && facts.starttime == Some(anchor.starttime) {
            return (Verdict::Match, Some(Verify::Anchored));
        }
        // The anchor names a different live incarnation: the instance
        // already has its daemon-launched tagma, so a second live
        // claimant must not be adopted over it (split brain, or a
        // runtime.json retargeted at someone else's process).
        if anchor.pid != pid && anchor_incarnation_live(anchor) {
            return (Verdict::Mismatch, None);
        }
    }
    if let Some(reported) = reported_starttime {
        if facts.starttime == Some(reported)
            && let Some(exe) = &facts.exe
            && tagma_exe_family(exe)
        {
            return (Verdict::Match, Some(Verify::Adopted));
        }
        if facts.exe.is_none() {
            // Live and self-reported, but the exe link is unreadable —
            // the same-uid proof adoption requires is missing.
            return (Verdict::Unknown, None);
        }
        // The self-report describes an incarnation other than the one
        // now on this pid: stale, reused, or rewritten.
        return (Verdict::Mismatch, None);
    }
    // No self-report credential on file (a pre-starttime runtime.json)
    // — the legacy chain: an anchor that pins this pid and can compare
    // starttimes decides, else the exe/comm name chain.
    if let Some(anchor) = anchored {
        if anchor.pid != pid {
            return (Verdict::Mismatch, None);
        }
        if anchor.starttime != 0
            && let Some(starttime) = facts.starttime
            && starttime != anchor.starttime
        {
            return (Verdict::Mismatch, None);
        }
    }
    let family_match = match (&facts.exe, &facts.comm) {
        (Some(exe), _) => tagma_exe_family(exe),
        (None, Some(comm)) => tagma_comm_matches(comm),
        (None, None) => return (Verdict::Unknown, None),
    };
    if family_match {
        (Verdict::Match, Some(Verify::ByName))
    } else {
        (Verdict::Mismatch, None)
    }
}

/// Whether the anchor's own claimed incarnation is still live. A live
/// pid whose starttime matches the anchor is the claiming process; an
/// alive-but-unverifiable pid (a legacy anchor without a starttime, or
/// an unreadable /proc) counts as claiming — adoption is refused
/// rather than risk a second live tagma on one directory.
fn anchor_incarnation_live(anchor: &Identity) -> bool {
    if !pid_is_alive(anchor.pid) {
        return false;
    }
    match proc_starttime(anchor.pid) {
        Some(starttime) => anchor.starttime != 0 && starttime == anchor.starttime,
        None => true,
    }
}
/// Classify a recorded pid against `anchored` and the runtime.json
/// self-report, warning exactly when the verdict is a degraded
/// by-name Match: the one "live, but only by name" case worth
/// investigating.
fn classify_with_facts(
    anchored: Option<&Identity>,
    reported_starttime: Option<u64>,
    pid: u32,
    slug: &str,
) -> (Verdict, Option<Verify>) {
    let facts = observe_identity(pid);
    let (verdict, verify) = classify_identity(anchored, reported_starttime, pid, &facts);
    if verify == Some(Verify::ByName) {
        tracing::warn!(
            slug = %slug,
            pid,
            comm = ?facts.comm,
            "pid matches tagma only by name (launch anchor missing or unverified)"
        );
    }
    (verdict, verify)
}
/// Classification for callers holding an instance dir rather than a
/// scanned struct: reads the anchor and the runtime self-report fresh
/// (the tree is the registry) and classifies the recorded pid.
pub fn identity_matches(instance_dir: &Path, pid: u32) -> Verdict {
    let anchored = read_meta(instance_dir).and_then(|meta| meta.identity);
    let reported = read_runtime(instance_dir)
        .and_then(|runtime| (runtime.starttime > 0).then_some(runtime.starttime));
    let slug = instance_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("?")
        .to_owned();
    classify_with_facts(anchored.as_ref(), reported, pid, &slug).0
}

/// `meta.json`: the managed-instance marker for an instance directory.
/// The daemon's spawn pipeline is the normal writer; a hand-written
/// minimal marker (instance_id + owner_uid) is the manual-launch
/// path. A directory without a parseable one is not managed. It
/// carries the static identity
/// (instance id, owning uid, canonical workspace, spawn-time user env); the volatile
/// runtime facts live in `runtime.json`.
/// `workspace` is the one optional key: a minimal hand-written marker
/// is enough — it just stops participating in overlap checks.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct InstanceMeta {
    pub instance_id: String,
    pub owner_uid: u32,
    #[serde(default)]
    pub workspace: Option<String>,
    /// The KEY=VALUE user env pairs the instance was spawned with,
    /// persisted so a stopped instance relaunches (via wire Start) with
    /// its original configuration. Daemon-owned keys are never stored
    /// here (they are re-derived per launch); #[serde(default)] keeps
    /// pre-field meta.json files parseable.
    #[serde(default)]
    pub env: Vec<String>,
    /// The launch claim anchor: the pid and kernel starttime of the
    /// exact process incarnation a launch verified as its own (see
    /// `Identity`). #[serde(default)] is generic tolerance for meta
    /// whose claim point could not pin one — tolerance, not a second
    /// accepted shape.
    #[serde(default)]
    pub identity: Option<Identity>,
}

/// `runtime.json`: the instance's runtime identity, written by the
/// tagma itself. The key set (`pid`, `port`, `starttime`) is a
/// cross-crate contract — kallip-tagma serializes its own mirror of
/// these keys and does not depend on the daemon crates, so the two
/// definitions stay in lockstep by hand. `starttime` is the writing
/// process's kernel start time: the adoption credential checked when
/// no launch anchor verifies the pid.
#[derive(Debug, serde::Deserialize)]
pub struct RuntimeFile {
    pub pid: u32,
    pub port: u16,
    /// `#[serde(default)]` keeps pre-field files parseable; `0` (or
    /// absent) means the file carries no adoption credential.
    #[serde(default)]
    pub starttime: u64,
}
/// Scan `<data_root>/*/meta.json`. Directories without the marker are
/// not managed — only marker-carrying directories are instances.
pub fn scan_instances(data_root: &Path) -> Vec<ScannedInstance> {
    let Ok(entries) = std::fs::read_dir(data_root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        let Some(slug) = entry.file_name().into_string().ok() else {
            continue;
        };
        let Some(meta) = read_meta(&dir) else {
            continue;
        };
        let runtime = read_runtime(&dir);
        let reported_starttime = runtime
            .as_ref()
            .and_then(|r| (r.starttime > 0).then_some(r.starttime));
        out.push(ScannedInstance {
            slug,
            instance_id: meta.instance_id,
            workspace: meta.workspace.filter(|w| !w.is_empty()),
            pid: runtime.as_ref().map(|r| r.pid),
            port: runtime.as_ref().map(|r| r.port),
            reported_starttime,
            owner: Some(meta.owner_uid),
            tagma_id: read_tagma_id(&dir),
            anchored: meta.identity,
        });
    }
    out.sort_by(|a, b| a.slug.cmp(&b.slug));
    out
}

pub(crate) fn read_meta(dir: &Path) -> Option<InstanceMeta> {
    let text = std::fs::read_to_string(dir.join("meta.json")).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn read_runtime(dir: &Path) -> Option<RuntimeFile> {
    let text = std::fs::read_to_string(dir.join("runtime.json")).ok()?;
    serde_json::from_str(&text).ok()
}

/// The enrolled tagma identity under `<instance>/credentials/`, read from
/// the tagma's own persisted `tagma.id`. Mirrors the tagma's primary-identity
/// rule as a conservative approximation: the first credentials entry
/// (alphabetical) carrying a non-empty `tagma.id` — single-archeion deployments
/// have exactly one entry, so this IS the tagma's primary. Discipline lock:
/// this reads `tagma.id` ONLY; `tagma.token` (0o600 secret) is never opened,
/// and the scan test asserts the token never reaches the wire.
/// The tagma's config order (relays.toml) is deliberately NOT read: that
/// file is the tagma process's own domain, while the instance tree is the
/// daemon's — the approximation stays within daemon-owned ground.
fn read_tagma_id(dir: &Path) -> Option<String> {
    let creds = dir.join("credentials");
    let Ok(entries) = std::fs::read_dir(&creds) else {
        return None;
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let is_dir = e.file_type().ok()?.is_dir();
            is_dir.then(|| e.file_name().into_string().ok()).flatten()
        })
        .collect();
    names.sort();
    for name in names {
        if let Ok(text) = std::fs::read_to_string(creds.join(name).join("tagma.id")) {
            let id = text.trim();
            if !id.is_empty() {
                return Some(id.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn write(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("fixture parent dir");
        }
        std::fs::write(path, text).expect("write fixture");
    }

    #[test]
    fn scan_skips_dirs_without_meta_and_sorts_by_slug() {
        let root = tempfile_dir("skips");
        write(
            &root.join("beta/meta.json"),
            r#"{"instance_id":"id-2","owner_uid":1001}"#,
        );
        write(
            &root.join("alpha/meta.json"),
            r#"{"instance_id":"id-1","owner_uid":1000,"workspace":"/tmp/w"}"#,
        );
        write(&root.join("broken/meta.json"), "{ not json");
        write(&root.join("noise-file"), "x"); // flat file: not a dir
        std::fs::create_dir(root.join("noise")).expect("noise dir");

        let scanned = scan_instances(&root);
        let slugs: Vec<_> = scanned.iter().map(|s| s.slug.as_str()).collect();
        assert_eq!(slugs, ["alpha", "beta"]);
        assert_eq!(scanned[0].instance_id, "id-1");
        assert_eq!(scanned[0].workspace.as_deref(), Some("/tmp/w"));
        assert_eq!(scanned[1].owner, Some(1001));
        assert_eq!(scanned[1].workspace, None);
        write(
            &root.join("empty/meta.json"),
            r#"{"instance_id":"id-3","owner_uid":1002,"workspace":""}"#,
        );
        // An empty-string workspace must read back as absent, not as a
        // zero-component path that overlaps everything.
        assert_eq!(scan_instances(&root)[2].workspace, None);
    }

    #[test]
    fn dead_pid_reports_not_running_with_detail() {
        let root = tempfile_dir("dead-pid");
        write(
            &root.join("a/meta.json"),
            r#"{"instance_id":"id","owner_uid":1000}"#,
        );
        write(&root.join("a/runtime.json"), r#"{"pid":99999999,"port":1}"#);
        let scanned = scan_instances(&root);
        assert_eq!(scanned[0].state(), InstanceState::Dead);
        let health = scanned[0].health();
        assert!(!health.running);
        assert_eq!(health.state, InstanceState::Dead);
        assert!(
            health.detail.unwrap().contains("pid"),
            "stale pid named in detail"
        );
    }

    #[test]
    fn no_runtime_file_means_not_running() {
        let root = tempfile_dir("no-pid");
        write(
            &root.join("a/meta.json"),
            r#"{"instance_id":"id","owner_uid":1000}"#,
        );
        let scanned = scan_instances(&root);
        let health = scanned[0].health();
        assert!(!health.running);
        assert_eq!(health.state, InstanceState::Stopped);
        assert_eq!(health.detail.as_deref(), Some("no runtime.json"));
    }

    #[test]
    fn live_foreign_process_is_not_a_match() {
        // This test binary is not kallip-tagma, so our own pid must NOT
        // count even though it is alive: the exe/comm fallback checks
        // are load-bearing, not decoration.
        let pid = std::process::id();
        let (verdict, verify) = classify_identity(None, None, pid, &observe_identity(pid));
        assert_eq!(verdict, Verdict::Mismatch);
        assert!(verify.is_none());
    }

    #[test]
    fn classify_matrix() {
        let anchor = Identity {
            pid: 42,
            starttime: 1000,
            anchored_at: 7,
        };
        let tagma_exe = Some("/nix/store/xyz-kallip-tagma-0.1.0/bin/kallip-tagma".to_string());
        let foreign_exe = Some("/usr/bin/sleep".to_string());
        let facts = |starttime: Option<u64>, exe: Option<String>, comm: Option<&str>| ProcFacts {
            starttime,
            exe,
            comm: comm.map(str::to_string),
        };
        // Anchor level: same pid, same starttime — the exact incarnation.
        let (v, how) = classify_identity(
            Some(&anchor),
            None,
            42,
            &facts(Some(1000), tagma_exe.clone(), Some("kallip-tagma")),
        );
        assert_eq!((v, how), (Verdict::Match, Some(Verify::Anchored)));
        // Same pid, different starttime: the pid was recycled.
        let (v, _) = classify_identity(
            Some(&anchor),
            None,
            42,
            &facts(Some(2000), tagma_exe.clone(), Some("kallip-tagma")),
        );
        assert_eq!(v, Verdict::Mismatch);
        // The runtime pid is not the anchored pid at all.
        let (v, _) = classify_identity(
            Some(&anchor),
            None,
            43,
            &facts(Some(1000), tagma_exe.clone(), Some("kallip-tagma")),
        );
        assert_eq!(v, Verdict::Mismatch);
        // Anchor present but starttime unreadable: falls below the
        // anchor; a family exe still matches, but by name.
        let (v, how) = classify_identity(
            Some(&anchor),
            None,
            42,
            &facts(None, tagma_exe.clone(), None),
        );
        assert_eq!((v, how), (Verdict::Match, Some(Verify::ByName)));
        // A hand-crafted zero starttime in the anchor cannot anchor
        // anything (the launch path never writes one): the anchor
        // level is skipped and the name chain decides.
        let zero_anchor = Identity {
            pid: 42,
            starttime: 0,
            anchored_at: 7,
        };
        let (v, how) = classify_identity(
            Some(&zero_anchor),
            None,
            42,
            &facts(Some(1000), tagma_exe.clone(), Some("kallip-tagma")),
        );
        assert_eq!((v, how), (Verdict::Match, Some(Verify::ByName)));
        // No anchor at all: name-chain matches are by-name matches.
        let (v, how) = classify_identity(
            None,
            None,
            42,
            &facts(Some(1000), tagma_exe.clone(), Some("kallip-tagma")),
        );
        assert_eq!((v, how), (Verdict::Match, Some(Verify::ByName)));
        // Wrapped binary: comm falls back to the truncated shim name.
        let (v, how) =
            classify_identity(None, None, 42, &facts(None, None, Some(".kallip-tagma-w")));
        assert_eq!((v, how), (Verdict::Match, Some(Verify::ByName)));
        // A readable exe that is not family decides against comm.
        let (v, _) = classify_identity(
            None,
            None,
            42,
            &facts(None, foreign_exe, Some("kallip-tagma")),
        );
        assert_eq!(v, Verdict::Mismatch);
        // Nothing readable on a live process: unverifiable.
        let (v, _) = classify_identity(None, None, 42, &facts(None, None, None));
        assert_eq!(v, Verdict::Unknown);
        // A dead pid is Gone — quiet, never verified (u32::MAX names no
        // process; kernel pids cap far below it).
        let (v, how) = classify_identity(None, None, u32::MAX, &ProcFacts::default());
        assert_eq!((v, how), (Verdict::Gone, None));
    }

    #[test]
    fn adoption_triple_verifies_a_self_reported_process() {
        // No anchor: the runtime.json self-report (starttime + family
        // exe) is enough to adopt the live process.
        let pid = std::process::id();
        let facts = ProcFacts {
            starttime: Some(4242),
            exe: Some("/nix/store/xyz-kallip-tagma-0.1.0/bin/kallip-tagma".to_string()),
            comm: None,
        };
        let (v, how) = classify_identity(None, Some(4242), pid, &facts);
        assert_eq!((v, how), (Verdict::Match, Some(Verify::Adopted)));
    }

    #[test]
    fn adoption_refused_when_the_report_describes_another_incarnation() {
        // The self-report names a starttime unlike the live pid's (pid
        // reuse, or a report rewritten at a stale incarnation): refused.
        let pid = std::process::id();
        let facts = ProcFacts {
            starttime: Some(9999),
            exe: Some("/nix/store/xyz-kallip-tagma-0.1.0/bin/kallip-tagma".to_string()),
            comm: Some("kallip-tagma".to_string()),
        };
        let (v, how) = classify_identity(None, Some(4242), pid, &facts);
        assert_eq!((v, how), (Verdict::Mismatch, None));
    }

    #[test]
    fn adoption_refused_without_a_readable_exe() {
        // starttime matches but the exe is unreadable (the cross-uid
        // shape): the same-uid proof is missing — Unknown, never Match.
        let pid = std::process::id();
        let facts = ProcFacts {
            starttime: Some(4242),
            exe: None,
            comm: Some("kallip-tagma".to_string()),
        };
        let (v, how) = classify_identity(None, Some(4242), pid, &facts);
        assert_eq!((v, how), (Verdict::Unknown, None));
    }

    #[test]
    fn adoption_proceeds_when_the_anchored_incarnation_is_gone() {
        // The anchored pid names no live process: a self-report on a
        // different live pid is adoption, not a conflict.
        let pid = std::process::id();
        let dead_anchor = Identity {
            pid: 99999999,
            starttime: 1000,
            anchored_at: 7,
        };
        let facts = ProcFacts {
            starttime: Some(4242),
            exe: Some("/nix/store/xyz-kallip-tagma-0.1.0/bin/kallip-tagma".to_string()),
            comm: None,
        };
        let (v, how) = classify_identity(Some(&dead_anchor), Some(4242), pid, &facts);
        assert_eq!((v, how), (Verdict::Match, Some(Verify::Adopted)));
    }

    #[test]
    fn adoption_denied_while_the_anchored_incarnation_is_live() {
        // The anchor's own incarnation is still live on its pid: a
        // second live claimant on the same directory is denied even
        // with a perfect self-report (split brain / retarget guard).
        let pid = std::process::id();
        let anchor = Identity {
            pid,
            starttime: proc_starttime(pid).expect("own /proc stat readable"),
            anchored_at: 7,
        };
        // A real live process stands in as the second claimant: the
        // guard fires for any live pid the anchor does not name.
        let mut sleeper = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("sleeper process");
        let claimant = sleeper.id();
        let facts = ProcFacts {
            starttime: Some(777),
            exe: Some("/nix/store/xyz-kallip-tagma-0.1.0/bin/kallip-tagma".to_string()),
            comm: None,
        };
        let (v, how) = classify_identity(Some(&anchor), Some(777), claimant, &facts);
        let _ = sleeper.kill();
        let _ = sleeper.wait();
        assert_eq!((v, how), (Verdict::Mismatch, None));
    }

    #[test]
    fn scan_of_a_non_tagma_self_report_stays_dead() {
        // This test binary publishes a perfect self-report — but its
        // exe is not a kallip-tagma, so adoption must refuse it.
        let root = tempfile_dir("adopt-refused");
        write(
            &root.join("a/meta.json"),
            r#"{"instance_id":"id","owner_uid":1000}"#,
        );
        let pid = std::process::id();
        let starttime = proc_starttime(pid).expect("own /proc stat readable");
        write(
            &root.join("a/runtime.json"),
            &format!(r#"{{"pid":{pid},"port":7000,"starttime":{starttime}}}"#),
        );
        let scanned = scan_instances(&root);
        assert_eq!(scanned[0].state(), InstanceState::Dead);
        assert!(!scanned[0].info().running);
    }

    #[test]
    fn pid_comm_reads_the_test_process_name() {
        let comm = pid_comm(std::process::id()).expect("own /proc entry readable");
        assert!(!comm.is_empty());
    }

    #[test]
    fn tagma_id_reads_first_entry_and_never_the_token() {
        let root = tempfile_dir("tagma-id");
        write(
            &root.join("team/meta.json"),
            r#"{"instance_id":"id-1","owner_uid":1000}"#,
        );
        // Two entries: alphabetical-first wins as the conservative primary.
        write(&root.join("team/credentials/b/tagma.id"), "tid-b\n");
        write(&root.join("team/credentials/a/tagma.id"), "tid-a\n");
        // The 0o600 secret sits next to the id; it must never surface.
        write(
            &root.join("team/credentials/a/tagma.token"),
            "sk-secret-token",
        );
        let info = scan_instances(&root)[0].info();
        assert_eq!(info.tagma_id.as_deref(), Some("tid-a"));
        let wire = serde_json::to_string(&info).expect("serialize InstanceInfo");
        assert!(!wire.contains("sk-secret-token"));
    }

    #[test]
    fn tagma_id_none_when_unenrolled_or_blank() {
        let root = tempfile_dir("tagma-id-none");
        write(
            &root.join("local/meta.json"),
            r#"{"instance_id":"id-2","owner_uid":1000}"#,
        ); // never enrolled: no credentials tree at all
        write(
            &root.join("blank/meta.json"),
            r#"{"instance_id":"id-3","owner_uid":1000}"#,
        );
        write(&root.join("blank/credentials/default/tagma.id"), "  \n");
        for scanned in scan_instances(&root) {
            assert_eq!(scanned.tagma_id, None);
        }
    }

    #[test]
    fn tagma_id_degrades_to_none_on_unreadable_credentials() {
        let root = tempfile_dir("tagma-id-unreadable");
        write(
            &root.join("u1/meta.json"),
            r#"{"instance_id":"id-4","owner_uid":1000}"#,
        );
        // `tagma.id` as a directory: the read fails and the entry is skipped.
        std::fs::create_dir_all(root.join("u1/credentials/default/tagma.id"))
            .expect("fixture dir-as-file");
        write(
            &root.join("u2/meta.json"),
            r#"{"instance_id":"id-5","owner_uid":1000}"#,
        );
        // `credentials` itself not a directory: the listing fails outright.
        write(&root.join("u2/credentials"), "not a dir");
        for scanned in scan_instances(&root) {
            assert_eq!(scanned.tagma_id, None);
        }
    }

    #[test]
    fn scan_surfaces_runtime_port() {
        let root = tempfile_dir("runtime-port");
        write(
            &root.join("a/meta.json"),
            r#"{"instance_id":"id-1","owner_uid":1000}"#,
        );
        write(&root.join("a/runtime.json"), r#"{"pid":1,"port":7301}"#);
        write(
            &root.join("b/meta.json"),
            r#"{"instance_id":"id-2","owner_uid":1000}"#,
        );
        let scanned = scan_instances(&root);
        // The raw scan keeps the runtime file's port (pid 1 is not a
        // live tagma, so this instance is NOT Running)...
        assert_eq!(scanned[0].port, Some(7301));
        assert_eq!(scanned[0].state(), InstanceState::Dead);
        // ...but the wire only surfaces a port for a live Running instance:
        // the dead runtime.json's stale port must never reach the wire.
        assert_eq!(scanned[0].info().port, None);
        // No runtime file at all: no port anywhere.
        assert_eq!(scanned[1].port, None);
    }

    fn tempfile_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("kallip-scan-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("tempdir");
        dir
    }
}

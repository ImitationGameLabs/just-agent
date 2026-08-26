//! Directory scan: the instance tree IS the registry.
//!
//! The daemon holds no state of its own — `list`/`health` read the tree under
//! the data root fresh on every call, so a daemon restart (or a crash)
//! rebuilds the full view from disk, and manually created directories are
//! adopted as long as they carry a daemon-written `meta.json`.

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
    /// The spawn-time uid of the requesting peer, from `meta.json`.
    pub owner: Option<u32>,
    /// The enrolled tagma identity (agora-issued id) if the instance's own
    /// credentials tree carries one; see `read_tagma_id`.
    pub tagma_id: Option<String>,
}

impl ScannedInstance {
    /// The daemon's three-way classification: a live tagma pid is
    /// Running, a missing pid file is a clean Stopped, and a recorded
    /// pid that no longer lives (or no longer looks like a tagma) is
    /// Dead. Single classification source — running() derives from it.
    pub fn state(&self) -> InstanceState {
        match self.pid {
            None => InstanceState::Stopped,
            Some(pid) if pid_is_tagma(pid) => InstanceState::Running,
            Some(_) => InstanceState::Dead,
        }
    }
    pub fn info(&self) -> InstanceInfo {
        let state = self.state();
        InstanceInfo {
            slug: self.slug.clone(),
            instance_id: self.instance_id.clone(),
            workspace: self.workspace.clone().unwrap_or_default(),
            running: state == InstanceState::Running,
            state,
            owner: self.owner,
            tagma_id: self.tagma_id.clone(),
        }
    }

    pub fn health(&self) -> HealthReport {
        let state = self.state();
        let detail = if state == InstanceState::Running {
            None
        } else if self.pid.is_none() {
            Some("no runtime.json".to_string())
        } else {
            Some("pid not alive (stale or reused)".to_string())
        };
        HealthReport {
            slug: Some(self.slug.clone()),
            running: state == InstanceState::Running,
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
/// True when `pid` is alive (not a zombie) and `/proc/<pid>/comm` starts
/// with `kallip-tagma`.
/// The prefix match (not equality) tolerates a 15-char comm truncation of
/// longer future names.
pub fn pid_is_tagma(pid: u32) -> bool {
    if !pid_is_alive(pid) {
        return false;
    }
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm"));
    matches!(comm, Ok(c) if c.trim_start_matches("kallip-").starts_with("tagma"))
}

/// `meta.json`: the adopt marker for an instance directory. The
/// daemon's spawn pipeline is the single writer; a directory without
/// a parseable one is not managed. It carries the static identity
/// (instance id, owning uid, canonical workspace); the volatile
/// runtime facts live in `runtime.json`.
/// `workspace` is the one optional key: a minimal hand-written marker
/// still adopts — it just stops participating in overlap checks.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct InstanceMeta {
    pub instance_id: String,
    pub owner_uid: u32,
    #[serde(default)]
    pub workspace: Option<String>,
}

/// `runtime.json`: the instance's runtime identity, written by the
/// tagma itself. The key set (`pid`, `port`) is a cross-crate
/// contract — kallip-tagma serializes its own mirror of these keys
/// and does not depend on the daemon crates, so the two definitions
/// stay in lockstep by hand.
#[derive(Debug, serde::Deserialize)]
pub struct RuntimeFile {
    pub pid: u32,
    pub port: u16,
}
/// Scan `<data_root>/*/meta.json`. Directories without the marker are
/// not managed (the legacy flat layout keeps running unmanaged).
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
        out.push(ScannedInstance {
            slug,
            instance_id: meta.instance_id,
            workspace: meta.workspace.filter(|w| !w.is_empty()),
            pid: read_runtime(&dir).map(|runtime| runtime.pid),
            owner: Some(meta.owner_uid),
            tagma_id: read_tagma_id(&dir),
        });
    }
    out.sort_by(|a, b| a.slug.cmp(&b.slug));
    out
}

fn read_meta(dir: &Path) -> Option<InstanceMeta> {
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
/// (alphabetical) carrying a non-empty `tagma.id` — single-agora deployments
/// have exactly one entry, so this IS the tagma's primary. Discipline lock:
/// this reads `tagma.id` ONLY; `tagma.token` (0o600 secret) is never opened,
/// and the scan test asserts the token never reaches the wire.
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
    fn live_pid_of_this_test_process_counts_as_tagma_via_comm() {
        // This test binary is not kallip-tagma, so our own pid must NOT
        // count even though it is alive: the comm check is load-bearing.
        assert!(!pid_is_tagma(std::process::id()));
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
        write(&root.join("team/credentials/a/tagma.token"), "sk-secret-token");
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

    fn tempfile_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("kallip-daemon-scan-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("tempdir");
        dir
    }
}

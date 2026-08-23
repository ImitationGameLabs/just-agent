//! Directory scan: the instance tree IS the registry.
//!
//! The daemon holds no state of its own — `list`/`health` read the tree under
//! the data root fresh on every call, so a daemon restart (or a crash)
//! rebuilds the full view from disk, and manually created directories are
//! adopted as long as they carry an `instance.id`.

use std::path::{Path, PathBuf};

use kallip_daemon_common::wire::{HealthReport, InstanceInfo};

/// One scanned instance directory: what the tree says, without judging it.
#[derive(Debug, Clone)]
pub struct ScannedInstance {
    pub slug: String,
    pub instance_id: String,
    pub workspace: Option<String>,
    /// The `pid` file content when present and parse-able.
    pub pid: Option<u32>,
    /// The `port` file content when present and parse-able.
    pub port: Option<u16>,
    /// The `owner` file content when present and parse-able (the
    /// spawn-time uid of the requesting peer).
    pub owner: Option<u32>,
}

impl ScannedInstance {
    /// Whether the recorded pid is alive and still looks like a tagma
    /// (`/proc/<pid>/comm` match guards against pid reuse).
    pub fn running(&self) -> bool {
        match self.pid {
            Some(pid) => pid_is_tagma(pid),
            None => false,
        }
    }

    pub fn info(&self) -> InstanceInfo {
        InstanceInfo {
            slug: self.slug.clone(),
            instance_id: self.instance_id.clone(),
            workspace: self.workspace.clone().unwrap_or_default(),
            running: self.running(),
            owner: self.owner,
        }
    }

    pub fn health(&self) -> HealthReport {
        let running = self.running();
        let detail = if running {
            None
        } else if self.pid.is_none() {
            Some("no pid file".to_string())
        } else {
            Some("pid not alive (stale or reused)".to_string())
        };
        HealthReport {
            slug: Some(self.slug.clone()),
            running,
            detail,
        }
    }
}

/// True when `pid` exists and `/proc/<pid>/comm` starts with `kallip-tagma`.
/// The prefix match (not equality) tolerates a 15-char comm truncation of
/// longer future names.
pub fn pid_is_tagma(pid: u32) -> bool {
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm"));
    matches!(comm, Ok(c) if c.trim_start_matches("kallip-").starts_with("tagma"))
}

/// Scan `<data_root>/*/instance.id`. Directories without `instance.id` are
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
        let Some(instance_id) = read_trimmed(&dir.join("instance.id")) else {
            continue;
        };
        out.push(ScannedInstance {
            slug,
            instance_id,
            workspace: read_trimmed(&dir.join("workspace")),
            pid: read_trimmed(&dir.join("pid")).and_then(|p| p.parse().ok()),
            port: read_trimmed(&dir.join("port")).and_then(|p| p.parse().ok()),
            owner: read_trimmed(&dir.join("owner")).and_then(|o| o.parse().ok()),
        });
    }
    out.sort_by(|a, b| a.slug.cmp(&b.slug));
    out
}

fn read_trimmed(path: &PathBuf) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("fixture parent dir");
        }
        std::fs::write(path, text).expect("write fixture");
    }

    #[test]
    fn scan_skips_dirs_without_instance_id_and_sorts_by_slug() {
        let root = tempfile_dir("skips");
        write(&root.join("beta/instance.id"), "id-2");
        write(&root.join("alpha/instance.id"), "id-1");
        write(&root.join("alpha/owner"), "1000");
        write(&root.join("workspace"), "id-x"); // legacy flat: not a dir
        std::fs::create_dir(root.join("noise")).expect("noise dir");

        let scanned = scan_instances(&root);
        let slugs: Vec<_> = scanned.iter().map(|s| s.slug.as_str()).collect();
        assert_eq!(slugs, ["alpha", "beta"]);
        assert_eq!(scanned[0].instance_id, "id-1");
        assert_eq!(scanned[0].owner, Some(1000));
        assert_eq!(scanned[1].owner, None);
    }

    #[test]
    fn dead_pid_reports_not_running_with_detail() {
        let root = tempfile_dir("dead-pid");
        write(&root.join("a/instance.id"), "id");
        write(&root.join("a/pid"), "99999999");
        let scanned = scan_instances(&root);
        assert!(!scanned[0].running());
        let health = scanned[0].health();
        assert!(!health.running);
        assert!(
            health.detail.unwrap().contains("pid"),
            "stale pid named in detail"
        );
    }

    #[test]
    fn no_pid_file_means_not_running() {
        let root = tempfile_dir("no-pid");
        write(&root.join("a/instance.id"), "id");
        let scanned = scan_instances(&root);
        let health = scanned[0].health();
        assert!(!health.running);
        assert_eq!(health.detail.as_deref(), Some("no pid file"));
    }

    #[test]
    fn live_pid_of_this_test_process_counts_as_tagma_via_comm() {
        // This test binary is not kallip-tagma, so our own pid must NOT
        // count even though it is alive: the comm check is load-bearing.
        assert!(!pid_is_tagma(std::process::id()));
    }

    fn tempfile_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("kallip-daemon-scan-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("tempdir");
        dir
    }
}

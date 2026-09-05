//! Liveness reconciliation: a slow background sweep that announces the moment
//! a running instance dies.
//!
//! The wire answers (`list`/`health`) already re-scan the tree on every call,
//! so the panel's view is always fresh; what the tree cannot do is *speak* —
//! nothing notices a Running→Dead transition unless someone happens to poll.
//! This loop is that watcher: it keeps its own previous-tick snapshot (the
//! daemon proper stays stateless — the tree remains the only truth), and on
//! one scan cycle's Running→Dead edge it emits a warn line carrying slug and
//! pid. Restarting the daemon resets the snapshot: the first tick after boot
//! observes without alerting, so pre-existing corpses don't fire.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use kallip_daemon_common::wire::InstanceState;

use crate::scan;

/// How often the reconcile loop re-scans the tree. The panel polls `list`
/// every few seconds anyway; this cadence bounds only how long an unwatched
/// death goes unannounced in the daemon log.
const RECONCILE_INTERVAL: Duration = Duration::from_secs(30);

/// Run the reconciliation sweep until the process exits: scan, diff against
/// the previous tick, log the dead edges, sleep, repeat. The snapshot lives
/// in this task alone; nothing else in the daemon reads or writes it.
pub async fn run(data_root: PathBuf) {
    let mut seen: HashMap<String, InstanceState> = HashMap::new();
    loop {
        // Derive states here (the live /proc checks) so `edge_reports` stays a
        // pure diff over (slug, state, pid) tuples -- unit-testable without
        // depending on the host's pid space.
        let now: Vec<(String, InstanceState, Option<u32>)> = scan::scan_instances(&data_root)
            .into_iter()
            .map(|i| (i.slug.clone(), i.state(), i.pid))
            .collect();
        for (slug, pid) in edge_reports(&mut seen, now) {
            tracing::warn!(
                slug = %slug,
                pid = ?pid,
                log = %instance_logs_dir(&data_root, dirs::state_dir().as_deref(), &slug).display(),
                "instance died: recorded pid is no longer a live kallip-tagma"
            );
        }
        tokio::time::sleep(RECONCILE_INTERVAL).await;
    }
}
/// Where an instance's log files live: the state tree mirrors the data
/// tree (`<state_home>/kallipai/tagmata/<slug>/logs`) because logs are
/// pure output residue kept outside the portable instance tree. When
/// the platform state home is undetermined, fall back to the in-tree
/// location so the diagnostic still points somewhere real.
fn instance_logs_dir(data_root: &Path, state_home: Option<&Path>, slug: &str) -> PathBuf {
    match state_home {
        Some(home) => home
            .join("kallipai")
            .join("tagmata")
            .join(slug)
            .join("logs"),
        None => data_root.join(slug).join("logs"),
    }
}

/// Diff the current tick's `(slug, state, pid)` tuples against `seen`,
/// returning a `(slug, pid)` pair for each instance whose recorded pid
/// just died; `seen` is left holding this tick's states for the next
/// call. Slugs whose directory vanished between ticks are forgotten, so
/// a re-created directory starts fresh instead of inheriting a ghost
/// history.
fn edge_reports(
    seen: &mut HashMap<String, InstanceState>,
    now: Vec<(String, InstanceState, Option<u32>)>,
) -> Vec<(String, Option<u32>)> {
    let mut reports = Vec::new();
    let mut live_slugs = HashSet::new();
    for (slug, state, pid) in now {
        live_slugs.insert(slug.clone());
        let was = seen.insert(slug.clone(), state);
        if was == Some(InstanceState::Running) && state == InstanceState::Dead {
            reports.push((slug, pid));
        }
    }
    seen.retain(|slug, _| live_slugs.contains(slug));
    reports
}

#[cfg(test)]
mod tests {
    use super::*;
    use kallip_daemon_common::wire::InstanceState::*;

    #[test]
    fn dead_instance_logs_point_at_the_state_tree() {
        let dir = instance_logs_dir(
            Path::new("/data/kallipai/tagmata"),
            Some(Path::new("/state/home")),
            "e2e",
        );
        assert_eq!(dir, PathBuf::from("/state/home/kallipai/tagmata/e2e/logs"));
    }

    #[test]
    fn without_a_state_home_the_logs_pointer_falls_back_in_tree() {
        let dir = instance_logs_dir(Path::new("/data/tagmata"), None, "e2e");
        assert_eq!(dir, PathBuf::from("/data/tagmata/e2e/logs"));
    }

    /// One observed instance at one tick.
    fn tick(
        slug: &str,
        state: InstanceState,
        pid: Option<u32>,
    ) -> (String, InstanceState, Option<u32>) {
        (slug.to_string(), state, pid)
    }

    /// A death between ticks reports exactly once; further Dead ticks stay
    /// silent (one corpse, one line).
    #[test]
    fn running_to_dead_reports_once() {
        let mut seen = HashMap::new();
        let up = vec![tick("a", Running, Some(7))];
        assert!(edge_reports(&mut seen, up).is_empty());
        let down = vec![tick("a", Dead, Some(7))];
        let reports = edge_reports(&mut seen, down);
        assert_eq!(reports, vec![("a".to_string(), Some(7))]);
        let again = vec![tick("a", Dead, Some(7))];
        assert!(edge_reports(&mut seen, again).is_empty());
    }

    /// A corpse already present at the first tick after boot is observed,
    /// not announced (no previous Running to die).
    #[test]
    fn first_tick_dead_is_silent() {
        let mut seen = HashMap::new();
        let corpse = vec![tick("a", Dead, Some(7))];
        assert!(edge_reports(&mut seen, corpse).is_empty());
    }

    /// Stopped never alerts, even across re-scans.
    #[test]
    fn stopped_never_alerts() {
        let mut seen = HashMap::new();
        for _ in 0..3 {
            let halt = vec![tick("a", Stopped, None)];
            assert!(edge_reports(&mut seen, halt).is_empty());
        }
    }

    /// A directory removed and re-added between ticks forgets its history:
    /// the re-created instance's first observed state never alerts.
    #[test]
    fn removed_slug_is_forgotten() {
        let mut seen = HashMap::new();
        let up = vec![tick("a", Running, Some(7))];
        let _ = edge_reports(&mut seen, up);
        let down = vec![tick("a", Dead, Some(7))];
        let _ = edge_reports(&mut seen, down);
        // Dir deleted: absent from this tick's list.
        let _ = edge_reports(&mut seen, vec![]);
        // Re-created already Dead (an adopted corpse): first sight -- silent.
        let reborn = vec![tick("a", Dead, Some(7))];
        assert!(edge_reports(&mut seen, reborn).is_empty());
    }

    /// Two independent instances die in the same tick: both report.
    #[test]
    fn simultaneous_deaths_all_report() {
        let mut seen = HashMap::new();
        let up = vec![tick("a", Running, Some(1)), tick("b", Running, Some(2))];
        let _ = edge_reports(&mut seen, up);
        let down = vec![tick("a", Dead, Some(1)), tick("b", Dead, Some(2))];
        let mut reports = edge_reports(&mut seen, down);
        reports.sort();
        assert_eq!(
            reports,
            vec![("a".to_string(), Some(1)), ("b".to_string(), Some(2))]
        );
    }
}

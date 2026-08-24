//! End-to-end lifecycle: the daemon spawns a REAL kallip-tagma binary via
//! the detach helper, health reads the self-written pid/port, stop lands
//! SIGTERM and the process exits. These tests need the sibling workspace
//! binaries — resolved like the tagma sandbox harness does (KALLIP_BIN_DIR
//! → CARGO_BIN_EXE_* → deps-parent → PATH), so they run green both under
//! `cargo test` and inside the dev container.
//! A package-scoped
//! `cargo build -p kallip-daemon` does NOT produce the sibling binaries:
//! build the workspace first or the resolve chain falls through to PATH
//! and these tests spuriously fail.

use std::path::PathBuf;
use std::time::Duration;

use kallip_daemon_client::DaemonClient;
use kallip_daemon_common::wire::{ErrorCode, InstanceState, OkPayload, RequestBody, ResponseBody};

// The daemon crate is a binary; integration tests cannot import it. The
// lifecycle surface under test is the four verbs over the wire, so the
// tests boot the real daemon binary as a subprocess.
struct DaemonProc {
    socket: PathBuf,
    child: std::process::Child,
    _data: PathBuf,
    data_dir: tempfile::TempDir,
}

fn resolve_bin(name: &str) -> PathBuf {
    if let Ok(dir) = std::env::var("KALLIP_BIN_DIR")
        && let p = std::path::Path::new(&dir).join(name)
        && p.is_file()
    {
        return p;
    }
    let var = format!("CARGO_BIN_EXE_{name}");
    if let Ok(p) = std::env::var(&var) {
        return PathBuf::from(p);
    }
    if let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(std::path::Path::to_path_buf))
        && exe_dir.ends_with("deps")
        && let Some(profile_dir) = exe_dir.parent()
    {
        let in_target = profile_dir.join(name);
        if in_target.is_file() {
            return in_target;
        }
    }
    PathBuf::from(name)
}

fn start_daemon() -> DaemonProc {
    let data_dir = tempfile::tempdir().expect("data tempdir");
    let state_dir = tempfile::tempdir().expect("state tempdir");
    let socket = state_dir.path().join("control.sock");
    let bin = resolve_bin("kallip-daemon");
    let mut child = std::process::Command::new(&bin)
        .env("KALLIP_DAEMON_DATA_DIR", data_dir.path())
        .env("KALLIP_STATE_DIR", state_dir.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn daemon");
    // Wait for the socket to appear.
    for _ in 0..100 {
        if socket.exists() {
            return DaemonProc {
                socket,
                child,
                _data: state_dir.keep(),
                data_dir,
            };
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // Give-up path: do not leak the daemon process.
    let _ = child.kill();
    let _ = child.wait();
    panic!("daemon socket never appeared");
}

impl Drop for DaemonProc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn expect_ok(
    response: Result<kallip_daemon_common::wire::Response, kallip_daemon_client::ClientError>,
) -> OkPayload {
    let response = response.expect("client exchange");
    match response.body {
        ResponseBody::Ok { payload } => payload,
        ResponseBody::Err { code, message } => {
            panic!("expected ok, got {code:?}: {message}")
        }
    }
}

#[test]
fn spawn_health_stop_round_trip() {
    let daemon = start_daemon();
    let client = DaemonClient::new(&daemon.socket);
    let workspace = tempfile::tempdir().expect("workspace tempdir");

    // Spawn a real tagma: direct/local mode (no relay env) — the relay
    // plan resolution fails fast with neither configured... which is a
    // boot failure, proving the rollback path too if it ever regresses.
    // Minimal boot env: operator token plus the LLM profile trio the
    // runtime requires before it serves (PROVIDER/MODEL/API_KEY).
    let spawn = tokio_block_on(client.call(RequestBody::Spawn {
        slug: "e2e".into(),
        workspace: workspace.path().display().to_string(),
        env: vec![
            "KALLIP_OPERATOR_TOKEN=test-op-token".into(),
            "KALLIP_LLM_PROVIDER=deepseek".into(),
            "KALLIP_LLM_MODEL=test-model".into(),
            "KALLIP_LLM_DEEPSEEK_API_KEY=test-key".into(),
        ],
    }));
    let OkPayload::Spawn { slug, pid, port } = expect_ok(spawn) else {
        panic!("expected spawn payload");
    };
    assert_eq!(slug, "e2e");
    assert!(port > 0, "real bound port");
    assert!(PathBuf::from(format!("/proc/{pid}")).exists(), "pid alive");

    // Health: running.
    let health = tokio_block_on(client.call(RequestBody::Health {
        slug: Some("e2e".into()),
    }));
    let OkPayload::Health { report } = expect_ok(health) else {
        panic!("expected health payload");
    };
    assert!(report.running, "spawned instance is running");
    assert_eq!(report.state, InstanceState::Running);

    // The instance dir carries the metadata the scan adopts.
    let instance_dir = daemon.data_dir.path().join("e2e");
    assert!(instance_dir.join("instance.id").exists());
    assert!(instance_dir.join("workspace").exists());
    // The spawn recorded the requesting peer (this test process) as owner.
    let owner = std::fs::read_to_string(instance_dir.join("owner")).expect("owner marker");
    assert_eq!(owner.trim(), unsafe { libc::getuid() }.to_string());

    // Stop: TERM grace.
    let stop = tokio_block_on(client.call(RequestBody::Stop { slug: "e2e".into() }));
    let OkPayload::Stop { slug: stopped } = expect_ok(stop) else {
        panic!("expected stop payload");
    };
    assert_eq!(stopped, "e2e");
    for _ in 0..100 {
        if !PathBuf::from(format!("/proc/{pid}")).exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        !PathBuf::from(format!("/proc/{pid}")).exists(),
        "tagma exited after stop"
    );

    // Health after stop: not running, but the instance dir persists
    // (adoption semantics — stop does not deregister).
    let after = tokio_block_on(client.call(RequestBody::Health {
        slug: Some("e2e".into()),
    }));
    let OkPayload::Health { report } = expect_ok(after) else {
        panic!("expected health payload");
    };
    assert!(!report.running);
    assert_eq!(report.state, InstanceState::Dead);
    assert!(instance_dir.exists(), "instance dir survives stop");
}

#[test]
fn spawn_rejects_slug_reuse_and_workspace_overlap() {
    let daemon = start_daemon();
    let client = DaemonClient::new(&daemon.socket);
    let workspace = tempfile::tempdir().expect("workspace");

    let first = tokio_block_on(client.call(RequestBody::Spawn {
        slug: "taken".into(),
        workspace: workspace.path().display().to_string(),
        env: vec![
            "KALLIP_OPERATOR_TOKEN=t".into(),
            "KALLIP_LLM_PROVIDER=deepseek".into(),
            "KALLIP_LLM_MODEL=test-model".into(),
            "KALLIP_LLM_DEEPSEEK_API_KEY=test-key".into(),
        ],
    }));
    assert!(matches!(
        first.expect("first spawn").body,
        ResponseBody::Ok { .. }
    ));
    // Stop it so the second spawn's liveness wait cannot leak.
    let _ = tokio_block_on(client.call(RequestBody::Stop {
        slug: "taken".into(),
    }));

    let reuse = tokio_block_on(client.call(RequestBody::Spawn {
        slug: "taken".into(),
        workspace: workspace.path().display().to_string(),
        env: vec![],
    }));
    match reuse.expect("reuse exchange").body {
        ResponseBody::Err { code, .. } => assert_eq!(code, ErrorCode::SlugTaken),
        other => panic!("expected slug_taken, got {other:?}"),
    }

    // Same workspace under a different slug = overlap.
    let overlap = tokio_block_on(client.call(RequestBody::Spawn {
        slug: "other".into(),
        workspace: workspace.path().display().to_string(),
        env: vec![],
    }));
    match overlap.expect("overlap exchange").body {
        ResponseBody::Err { code, .. } => assert_eq!(code, ErrorCode::WorkspaceOverlap),
        other => panic!("expected workspace_overlap, got {other:?}"),
    }
}

/// Drive a tokio client call from a sync test: a minimal single-thread
/// runtime per call. (The daemon crate is async; the tests are not.)
fn tokio_block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime")
        .block_on(future)
}

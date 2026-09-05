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
    log_path: PathBuf,
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
    // stdout+stderr land in a file (not null) so tests can assert on
    // the daemon's own log — the quiet-Dead guarantee is a log claim.
    let log_path = state_dir.path().join("daemon.log");
    let log = std::fs::File::create(&log_path).expect("create daemon log");
    let mut child = std::process::Command::new(&bin)
        .env("KALLIP_DAEMON_DATA_DIR", data_dir.path())
        .env("KALLIP_STATE_DIR", state_dir.path())
        .stdout(log.try_clone().expect("clone log handle"))
        .stderr(log)
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
                log_path,
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
    assert!(instance_dir.join("meta.json").exists());
    for retired in ["instance.id", "owner", "pid", "port", "workspace"] {
        assert!(
            !instance_dir.join(retired).exists(),
            "retired marker {retired} must not appear"
        );
    }
    // The spawn recorded the requesting peer (this test process) as owner.
    let meta: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(instance_dir.join("meta.json")).expect("meta.json"),
    )
    .expect("parse meta.json");
    assert_eq!(
        meta["owner_uid"],
        serde_json::json!(unsafe { libc::getuid() })
    );
    assert_eq!(
        meta["workspace"],
        serde_json::json!(workspace.path().display().to_string())
    );
    assert!(
        meta["instance_id"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
    );
    // The launch claim point pinned the kernel incarnation: the anchor
    // names this pid with a real start time and a wall-clock stamp.
    let identity = &meta["identity"];
    assert_eq!(
        identity["pid"],
        serde_json::json!(pid),
        "anchor names the spawned pid"
    );
    assert!(
        identity["starttime"].as_u64().is_some_and(|t| t > 0),
        "anchor carries a kernel start time"
    );
    assert!(
        identity["anchored_at"].as_u64().is_some_and(|t| t > 0),
        "anchor carries its wall-clock stamp"
    );

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

    // Start: relaunch from the surviving tree. The fresh pid proves a new
    // process (not the old one lingering); the health gate re-opens.
    let started = tokio_block_on(client.call(RequestBody::Start {
        slug: "e2e".into(),
        env: vec!["KALLIP_TAGMA_LOG_TO_STDERR=1".into()],
    }));
    let OkPayload::Spawn {
        slug: started_slug,
        pid: started_pid,
        port: started_port,
    } = expect_ok(started)
    else {
        panic!("expected start payload");
    };
    assert_eq!(started_slug, "e2e");
    assert_ne!(started_pid, pid, "a fresh incarnation, not the old one");
    assert!(started_port > 0, "fresh bound port");

    // The survived tree carried the spawn-time env over (relaunch config),
    // and credentials/ persists so the enrolled identity revives.
    let meta_after: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(instance_dir.join("meta.json")).expect("meta after"),
    )
    .expect("parse meta.json after start");
    assert_eq!(
        meta_after["env"][0],
        serde_json::json!("KALLIP_OPERATOR_TOKEN=test-op-token")
    );
    assert_eq!(
        meta_after["env"].as_array().expect("env array").len(),
        4,
        "one-shot overlay is not persisted to meta.json"
    );
    // The relaunch re-anchored: the previous incarnation's anchor was
    // overwritten with the fresh pid at the claim point.
    assert_eq!(
        meta_after["identity"]["pid"],
        serde_json::json!(started_pid),
        "relaunch re-anchored to the fresh incarnation"
    );
    assert!(
        instance_dir.join("credentials").exists(),
        "credentials survive"
    );

    // Starting the now-running instance again is the conflict case.
    let code = match tokio_block_on(client.call(RequestBody::Start {
        slug: "e2e".into(),
        env: Vec::new(),
    }))
    .expect("double start response")
    .body
    {
        ResponseBody::Err { code, .. } => code,
        other => panic!("expected slug_taken conflict, got {other:?}"),
    };
    assert_eq!(code, ErrorCode::SlugTaken);

    // Cleanup so the test does not leave a live tagma behind.
    let stopped_again = tokio_block_on(client.call(RequestBody::Stop { slug: "e2e".into() }));
    let OkPayload::Stop {
        slug: stopped_again_slug,
    } = expect_ok(stopped_again)
    else {
        panic!("expected stop payload");
    };
    assert_eq!(stopped_again_slug, "e2e");

    // Quiet-Dead: after the graceful exit, repeated polls classify
    // the dead pid as Gone with zero degraded-match warnings — a dead
    // pid must never trip the "matches only by name" warn.
    for _ in 0..2 {
        let poll = tokio_block_on(client.call(RequestBody::Health {
            slug: Some("e2e".into()),
        }));
        let OkPayload::Health { report } = expect_ok(poll) else {
            panic!("expected health payload");
        };
        assert_eq!(report.state, InstanceState::Dead);
    }
    let log = std::fs::read_to_string(&daemon.log_path).expect("daemon log");
    assert!(
        !log.contains("matches tagma only by name"),
        "degraded-match warn must stay silent on a healthy lifecycle:\n{log}"
    );
}

#[test]
fn start_filters_consumed_enrollment_code() {
    let daemon = start_daemon();
    let client = DaemonClient::new(&daemon.socket);
    let workspace = tempfile::tempdir().expect("workspace tempdir");

    // Spawn local-only (the real enrolled setup is fabricated below — the
    // test has no archeion).
    let spawn = tokio_block_on(client.call(RequestBody::Spawn {
        slug: "stale-code".into(),
        workspace: workspace.path().display().to_string(),
        env: vec![
            "KALLIP_OPERATOR_TOKEN=test-op-token".into(),
            "KALLIP_LLM_PROVIDER=deepseek".into(),
            "KALLIP_LLM_MODEL=test-model".into(),
            "KALLIP_LLM_DEEPSEEK_API_KEY=test-key".into(),
        ],
    }));
    let OkPayload::Spawn { pid, .. } = expect_ok(spawn) else {
        panic!("expected spawn payload");
    };

    // Stop and wait for the exit, so Start relaunches rather than
    // conflicting with a live instance.
    let stop = tokio_block_on(client.call(RequestBody::Stop {
        slug: "stale-code".into(),
    }));
    expect_ok(stop);
    for _ in 0..100 {
        if !PathBuf::from(format!("/proc/{pid}")).exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    // Fabricate the bug's exact state: a spawn-time enrollment code still
    // persisted in meta.json plus credentials stored by a completed
    // enrollment. The archeion points at a port nothing listens on — after the
    // fix the real tagma boots through the Stored branch and the entry
    // merely degrades to local-only; replaying the code instead makes tagma
    // fail fast on stored-credentials-plus-code and Start times out.
    let instance_dir = daemon.data_dir.path().join("stale-code");
    let mut meta: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(instance_dir.join("meta.json")).expect("meta.json"),
    )
    .expect("parse meta.json");
    let env = meta["env"].as_array_mut().expect("env array");
    env.push("KALLIP_TAGMA_RELAY_ARCHEION_URL=http://127.0.0.1:9".into());
    env.push("KALLIP_TAGMA_RELAY_ENROLLMENT_CODE=sk-spent".into());
    std::fs::write(
        instance_dir.join("meta.json"),
        serde_json::to_string(&meta).expect("serialize meta"),
    )
    .expect("rewrite meta.json");
    let entry = instance_dir.join("credentials").join("default");
    std::fs::create_dir_all(&entry).expect("create credentials entry");
    std::fs::write(entry.join("tagma.id"), "tagma-1").expect("write tagma.id");
    std::fs::write(entry.join("tagma.token"), "token").expect("write tagma.token");

    let started = tokio_block_on(client.call(RequestBody::Start {
        slug: "stale-code".into(),
        env: Vec::new(),
    }));
    let OkPayload::Spawn {
        pid: started_pid,
        port: started_port,
        ..
    } = expect_ok(started)
    else {
        panic!("expected start payload");
    };
    assert!(started_port > 0, "fresh bound port");
    assert_ne!(started_pid, pid, "a fresh incarnation");

    // The scrub removed the spent code from the persisted copy while the
    // rest of the env survived (the archeion url is not secret material and
    // stays — the Stored branch needs it).
    let after: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(instance_dir.join("meta.json")).expect("meta after"),
    )
    .expect("parse meta.json after");
    let env = after["env"].as_array().expect("env array after");
    assert!(env.contains(&serde_json::json!("KALLIP_OPERATOR_TOKEN=test-op-token")));
    assert!(env.contains(&serde_json::json!(
        "KALLIP_TAGMA_RELAY_ARCHEION_URL=http://127.0.0.1:9"
    )));
    assert!(!env.iter().any(|pair| {
        pair.as_str()
            .is_some_and(|p| p.starts_with("KALLIP_TAGMA_RELAY_ENROLLMENT_CODE="))
    }));

    // Cleanup so the test does not leave a live tagma behind.
    let stop = tokio_block_on(client.call(RequestBody::Stop {
        slug: "stale-code".into(),
    }));
    expect_ok(stop);
    for _ in 0..100 {
        if !PathBuf::from(format!("/proc/{started_pid}")).exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        !PathBuf::from(format!("/proc/{started_pid}")).exists(),
        "relaunched tagma exited after stop"
    );
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

#[test]
fn start_recovers_from_stale_runtime_json() {
    // The regression this locks: a leftover runtime.json from a previous
    // incarnation used to poison the launch poll, and its recorded pid
    // (required to look like a tagma before it was trusted) decided
    // between a bogus adoption and a 30s kill. The launch now clears the
    // leftover before starting the helper, so the poll only ever sees
    // this launch's self-report.
    let daemon = start_daemon();
    let client = DaemonClient::new(&daemon.socket);
    let workspace = tempfile::tempdir().expect("workspace tempdir");

    let spawn = tokio_block_on(client.call(RequestBody::Spawn {
        slug: "stale-runtime".into(),
        workspace: workspace.path().display().to_string(),
        env: vec![
            "KALLIP_OPERATOR_TOKEN=test-op-token".into(),
            "KALLIP_LLM_PROVIDER=deepseek".into(),
            "KALLIP_LLM_MODEL=test-model".into(),
            "KALLIP_LLM_DEEPSEEK_API_KEY=test-key".into(),
        ],
    }));
    let OkPayload::Spawn { pid, .. } = expect_ok(spawn) else {
        panic!("expected spawn payload");
    };

    // Stop and wait for the exit, so the fabricated runtime.json below
    // carries a genuinely dead pid — exactly what a crash leaves behind.
    let stop = tokio_block_on(client.call(RequestBody::Stop {
        slug: "stale-runtime".into(),
    }));
    expect_ok(stop);
    for _ in 0..100 {
        if !PathBuf::from(format!("/proc/{pid}")).exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let stale = serde_json::json!({ "pid": pid, "port": 1 });
    let instance_dir = daemon.data_dir.path().join("stale-runtime");
    std::fs::write(
        instance_dir.join("runtime.json"),
        serde_json::to_vec(&stale).expect("serialize stale runtime"),
    )
    .expect("write stale runtime.json");

    let started = tokio_block_on(client.call(RequestBody::Start {
        slug: "stale-runtime".into(),
        env: Vec::new(),
    }));
    let OkPayload::Spawn {
        pid: started_pid,
        port: started_port,
        ..
    } = expect_ok(started)
    else {
        panic!("expected start payload");
    };
    assert_ne!(started_pid, pid, "the stale pid, not this launch");
    assert!(started_port > 0, "fresh bound port");

    // Cleanup so the test does not leave a live tagma behind.
    let stop = tokio_block_on(client.call(RequestBody::Stop {
        slug: "stale-runtime".into(),
    }));
    expect_ok(stop);
    for _ in 0..100 {
        if !PathBuf::from(format!("/proc/{started_pid}")).exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        !PathBuf::from(format!("/proc/{started_pid}")).exists(),
        "relaunched tagma exited after stop"
    );
}

#[test]
fn start_rejects_when_stale_runtime_names_a_live_pid() {
    // Liveness alone decides AlreadyRunning: a leftover naming a live pid
    // (here: this test process) is refused, not adopted and not killed.
    // Under the old comm re-check this same shape wedged the poll into a
    // 30s timeout whose kill branch SIGKILLed this very pid.
    let daemon = start_daemon();
    let client = DaemonClient::new(&daemon.socket);
    let workspace = tempfile::tempdir().expect("workspace tempdir");

    let spawn = tokio_block_on(client.call(RequestBody::Spawn {
        slug: "live-stale".into(),
        workspace: workspace.path().display().to_string(),
        env: vec![
            "KALLIP_OPERATOR_TOKEN=test-op-token".into(),
            "KALLIP_LLM_PROVIDER=deepseek".into(),
            "KALLIP_LLM_MODEL=test-model".into(),
            "KALLIP_LLM_DEEPSEEK_API_KEY=test-key".into(),
        ],
    }));
    let OkPayload::Spawn { pid, .. } = expect_ok(spawn) else {
        panic!("expected spawn payload");
    };

    let stop = tokio_block_on(client.call(RequestBody::Stop {
        slug: "live-stale".into(),
    }));
    expect_ok(stop);
    for _ in 0..100 {
        if !PathBuf::from(format!("/proc/{pid}")).exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    // A live pid that is not a tagma: the conservative refusal is the
    // point. Refusing costs one retry; adopting or killing an unrelated
    // process costs the process.
    let stale = serde_json::json!({ "pid": std::process::id(), "port": 1 });
    let instance_dir = daemon.data_dir.path().join("live-stale");
    std::fs::write(
        instance_dir.join("runtime.json"),
        serde_json::to_vec(&stale).expect("serialize stale runtime"),
    )
    .expect("write stale runtime.json");

    let started = tokio_block_on(client.call(RequestBody::Start {
        slug: "live-stale".into(),
        env: Vec::new(),
    }));
    let response = started.expect("client exchange");
    match response.body {
        ResponseBody::Err { code, .. } => assert_eq!(code, ErrorCode::SlugTaken),
        other => panic!("expected slug_taken refusal, got {other:?}"),
    }
}

/// A manually booted tagma on an unmarked data root must stay unwritten:
/// no meta.json means no daemon management, so no runtime.json. The
/// gate runs between bind and serve, so a successful TCP connect to the
/// fixed addr proves the gate already decided — no sleep needed.
#[test]
fn manual_boot_in_unmarked_dir_writes_nothing() {
    let data_dir = tempfile::tempdir().expect("data tempdir");
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
    let port = probe.local_addr().expect("probe addr").port();
    drop(probe);
    let mut tagma = std::process::Command::new(resolve_bin("kallip-tagma"))
        .env("KALLIP_DATA_DIR", data_dir.path())
        .env("KALLIP_TAGMA_ADDR", format!("127.0.0.1:{port}"))
        .env("KALLIP_OPERATOR_TOKEN", "test-op-token")
        .env("KALLIP_LLM_PROVIDER", "deepseek")
        .env("KALLIP_LLM_MODEL", "test-model")
        .env("KALLIP_LLM_DEEPSEEK_API_KEY", "test-key")
        // Test-env isolation: a machine running inside the kallipai
        // stack (e.g. an agent) carries ambient KALLIP_TAGMA_RELAY_*
        // vars; leaked into the child they trip the tagma relay
        // fail-fast and this local-only boot never listens.
        .env_remove("KALLIP_TAGMA_RELAY_ARCHEION_URL")
        .env_remove("KALLIP_TAGMA_RELAY_LESCHE_URL")
        .env_remove("KALLIP_TAGMA_RELAY_ENROLLMENT_CODE")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("manual tagma boot");
    let mut connected = false;
    for _ in 0..200 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            connected = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = tagma.kill();
    let _ = tagma.wait();
    assert!(connected, "manual tagma never listened on {port}");
    assert!(
        !data_dir.path().join("runtime.json").exists(),
        "unmarked data root must not get a runtime.json"
    );
}

/// The stop guard's two legs: a runtime.json retargeted at a foreign
/// live pid is refused even though the pid is alive (the anchor names
/// a different incarnation), and once the tree is restored the same
/// stop succeeds because the anchor verifies the original pid.
#[test]
fn stop_refuses_tampered_runtime_pid_then_allows_restored() {
    let daemon = start_daemon();
    let client = DaemonClient::new(&daemon.socket);
    let workspace = tempfile::tempdir().expect("workspace tempdir");

    let spawn = tokio_block_on(client.call(RequestBody::Spawn {
        slug: "tamper".into(),
        workspace: workspace.path().display().to_string(),
        env: vec![
            "KALLIP_OPERATOR_TOKEN=test-op-token".into(),
            "KALLIP_LLM_PROVIDER=deepseek".into(),
            "KALLIP_LLM_MODEL=test-model".into(),
            "KALLIP_LLM_DEEPSEEK_API_KEY=test-key".into(),
        ],
    }));
    let OkPayload::Spawn { pid, port, .. } = expect_ok(spawn) else {
        panic!("expected spawn payload");
    };
    let instance_dir = daemon.data_dir.path().join("tamper");
    let runtime_path = instance_dir.join("runtime.json");
    let original = std::fs::read_to_string(&runtime_path).expect("read runtime.json");

    // Tamper: point runtime.json at this test process — alive, but
    // provably not the anchored incarnation.
    let tampered = serde_json::json!({ "pid": std::process::id(), "port": port });
    std::fs::write(
        &runtime_path,
        serde_json::to_vec(&tampered).expect("serialize tampered runtime"),
    )
    .expect("write tampered runtime.json");
    let refused = tokio_block_on(client.call(RequestBody::Stop {
        slug: "tamper".into(),
    }));
    match refused.expect("refused exchange").body {
        ResponseBody::Err { code, .. } => assert_eq!(code, ErrorCode::NotRunning),
        other => panic!("expected not_running refusal, got {other:?}"),
    }
    // The refusal must not have killed the real tagma.
    assert!(
        PathBuf::from(format!("/proc/{pid}")).exists(),
        "real tagma alive after refused stop"
    );

    // Restore: the anchor verifies the original pid and stop lands.
    std::fs::write(&runtime_path, original).expect("restore runtime.json");
    let stopped = tokio_block_on(client.call(RequestBody::Stop {
        slug: "tamper".into(),
    }));
    expect_ok(stopped);
    for _ in 0..100 {
        if !PathBuf::from(format!("/proc/{pid}")).exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        !PathBuf::from(format!("/proc/{pid}")).exists(),
        "tagma exited after restored stop"
    );
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

//! Full-stack round trip against a REAL daemon subprocess: the router
//! (token + host guards + proxy + mapping) driven end to end over oneshot,
//! with the daemon answering on a tempdir UDS socket.
//!
//! The spawn leg launches a real tagma binary, so these tests need the
//! sibling workspace binaries — resolved the same way the daemon's own
//! lifecycle tests do (KALLIP_BIN_DIR → CARGO_BIN_EXE_* → deps-parent →
//! PATH). Build the workspace (or at least `cargo build -p kallip-daemon
//! kallip-daemon-web kallip`) before running, or the resolve chain falls
//! through to PATH and the tests spuriously fail.

use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use kallip_daemon_client::DaemonClient;
use kallip_daemon_web::{AppState, build_router};
use tower::ServiceExt;

struct DaemonProc {
    socket: PathBuf,
    child: std::process::Child,
    _state: PathBuf,
}

fn resolve_bin(name: &str) -> PathBuf {
    if let Ok(dir) = std::env::var("KALLIP_BIN_DIR")
        && let p = Path::new(&dir).join(name)
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
        .and_then(|e| e.parent().map(Path::to_path_buf))
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
        .env("KALLIP_DATA_DIR", data_dir.path())
        .env("KALLIP_STATE_DIR", state_dir.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn daemon");
    for _ in 0..100 {
        if socket.exists() {
            // The daemon adopts the data dir; keep both tempdirs alive by
            // leaking them (test-scoped, under /tmp).
            std::mem::forget(data_dir);
            std::mem::forget(state_dir);
            return DaemonProc {
                socket,
                child,
                _state: PathBuf::new(),
            };
        }
        std::thread::sleep(Duration::from_millis(50));
    }
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

async fn send(
    app: &axum::Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<&str>,
) -> (StatusCode, String) {
    let mut builder = Request::builder().method(method).uri(uri);
    // A Host is mandatory here: the router's host guard rejects requests
    // without one, exactly as a real browser would send it.
    builder = builder.header("host", "127.0.0.1:7300");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Body::from(body.unwrap_or("").to_string()))
        .unwrap();
    let response = app.clone().oneshot(request).await.expect("send");
    let status = response.status();
    let bytes = response.into_body().collect().await.expect("read body");
    (
        status,
        String::from_utf8(bytes.to_bytes().to_vec()).expect("utf8"),
    )
}

#[tokio::test]
async fn full_management_round_trip_with_guards() {
    let daemon = start_daemon();
    // Drain the daemon's startup backlog with one direct exchange so the
    // first proxied call is not racing the listener.
    let probe = DaemonClient::new(&daemon.socket);
    let _ = probe
        .call(kallip_daemon_common::wire::RequestBody::List)
        .await;

    let state = AppState {
        client: DaemonClient::new(&daemon.socket),
        token: "itest-token".into(),
    };
    let app = build_router(state, None);

    // No token: 401.
    let (status, body) = send(&app, "GET", "/api/daemon/list", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body.contains("\"unauthorized\""), "{body}");

    // Foreign Host: 403, even with a valid token.
    let request = Request::get("/api/daemon/list")
        .header("host", "evil.example")
        .header("authorization", "Bearer itest-token")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.expect("send");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    // Health (daemon itself): 200.
    let (status, body) = send(&app, "GET", "/api/daemon/health", Some("itest-token"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("\"running\":true"), "{body}");

    // Spawn a real instance (minimal boot env from the daemon lifecycle
    // tests: operator token + the LLM profile trio).
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    let spawn_body = serde_json::json!({
        "slug": "web-e2e",
        "workspace": workspace.path().display().to_string(),
        "env": [
            "KALLIP_OPERATOR_TOKEN=test-op-token",
            "KALLIP_LLM_PROVIDER=deepseek",
            "KALLIP_LLM_MODEL=test-model",
            "KALLIP_LLM_DEEPSEEK_API_KEY=test-key",
        ],
    })
    .to_string();
    let (status, body) = send(
        &app,
        "POST",
        "/api/daemon/spawn",
        Some("itest-token"),
        Some(&spawn_body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("\"slug\":\"web-e2e\""), "{body}");
    assert!(body.contains("\"pid\":"), "{body}");
    assert!(body.contains("\"port\":"), "{body}");

    // List sees it.
    let (status, body) = send(&app, "GET", "/api/daemon/list", Some("itest-token"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("\"web-e2e\""), "{body}");

    // Health for the slug: running.
    let (status, body) = send(
        &app,
        "GET",
        "/api/daemon/health?slug=web-e2e",
        Some("itest-token"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("\"running\":true"), "{body}");

    // Unknown slug: the daemon's not_found maps to 404 with the code key.
    let (status, body) = send(
        &app,
        "GET",
        "/api/daemon/health?slug=missing",
        Some("itest-token"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(body.contains("\"not_found\""), "{body}");

    // Stop it.
    let (status, body) = send(
        &app,
        "POST",
        "/api/daemon/stop",
        Some("itest-token"),
        Some(r#"{"slug":"web-e2e"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("\"web-e2e\""), "{body}");

    // Stop again: not_running maps to 409.
    let (status, body) = send(
        &app,
        "POST",
        "/api/daemon/stop",
        Some("itest-token"),
        Some(r#"{"slug":"web-e2e"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body.contains("\"not_running\""), "{body}");
}

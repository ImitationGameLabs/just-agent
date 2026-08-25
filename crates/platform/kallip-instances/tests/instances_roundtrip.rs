//! Full-stack round trip against a REAL daemon subprocess: the router
//! (token + host guards + proxy + mapping) driven end to end over oneshot,
//! with the daemon answering on a tempdir UDS socket.
//!
//! The spawn leg launches a real tagma binary, so these tests need the
//! sibling workspace binaries — resolved the same way the daemon's own
//! lifecycle tests do (KALLIP_BIN_DIR → CARGO_BIN_EXE_* → deps-parent →
//! PATH). Build the workspace (or at least `cargo build -p kallip-daemon
//! kallip-instances kallip`) before running, or the resolve chain falls
//! through to PATH and the tests spuriously fail.

use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use kallip_daemon_client::DaemonClient;
use kallip_instances::{AppState, build_router};
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
        .env("KALLIP_DAEMON_DATA_DIR", data_dir.path())
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
        backend: kallip_instances::backend::UdsBackend::arc(DaemonClient::new(&daemon.socket)),
        auth: kallip_instances::guard::AuthMode::Token("itest-token".into()),
        cors_origins: String::new(),
        allowed_hosts: vec![],
    };
    let app = build_router(state);

    // No token: 401.
    let (status, body) = send(&app, "GET", "/api/instances/list", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body.contains("\"unauthorized\""), "{body}");
    // Capabilities sits behind the same guard: no token, no list.
    let (status, _body) = send(&app, "GET", "/api/instances/capabilities", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Foreign Host: 403, even with a valid token.
    let request = Request::get("/api/instances/list")
        .header("host", "evil.example")
        .header("authorization", "Bearer itest-token")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.expect("send");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    // Health (daemon itself): 200.
    let (status, body) = send(
        &app,
        "GET",
        "/api/instances/health",
        Some("itest-token"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("\"running\":true"), "{body}");
    assert!(body.contains("\"state\":\"running\""), "{body}");

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
        "/api/instances/spawn",
        Some("itest-token"),
        Some(&spawn_body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("\"slug\":\"web-e2e\""), "{body}");
    assert!(body.contains("\"pid\":"), "{body}");
    assert!(body.contains("\"port\":"), "{body}");
    // An advertised-method fetch and an unsupported spawn method both
    // speak the capability vocabulary.
    let (status, body) = send(
        &app,
        "GET",
        "/api/instances/capabilities",
        Some("itest-token"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("designated-user"), "{body}");
    let bad_method = serde_json::json!({
        "slug": "web-e2e",
        "workspace": "/tmp/itest",
        "method": "container",
    })
    .to_string();
    let (status, body) = send(
        &app,
        "POST",
        "/api/instances/spawn",
        Some("itest-token"),
        Some(&bad_method),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.contains("unsupported_method"), "{body}");

    // List sees it.
    let (status, body) = send(
        &app,
        "GET",
        "/api/instances/list",
        Some("itest-token"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("\"web-e2e\""), "{body}");

    // Health for the slug: running.
    let (status, body) = send(
        &app,
        "GET",
        "/api/instances/health?slug=web-e2e",
        Some("itest-token"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("\"running\":true"), "{body}");
    assert!(body.contains("\"state\":\"running\""), "{body}");

    // Unknown slug: the daemon's not_found maps to 404 with the code key.
    let (status, body) = send(
        &app,
        "GET",
        "/api/instances/health?slug=missing",
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
        "/api/instances/stop",
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
        "/api/instances/stop",
        Some("itest-token"),
        Some(r#"{"slug":"web-e2e"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body.contains("\"not_running\""), "{body}");
}

/// A scriptable stand-in for the agora verifier: each call consumes the next
/// programmed outcome.
struct MockVerifier {
    outcomes: std::sync::Mutex<
        Vec<
            Result<
                Option<kallip_agora_common::principal::Principal>,
                kallip_agora_common::control_plane::ControlPlaneError,
            >,
        >,
    >,
}

#[async_trait::async_trait]
impl kallip_instances::control_plane::BearerVerifier for MockVerifier {
    async fn verify_bearer(
        &self,
        _token: &str,
    ) -> Result<
        Option<kallip_agora_common::principal::Principal>,
        kallip_agora_common::control_plane::ControlPlaneError,
    > {
        self.outcomes
            .lock()
            .unwrap()
            .pop()
            .expect("programmed outcome")
    }
}

#[tokio::test]
async fn platform_mode_admin_only_and_fail_closed() {
    use kallip_agora_common::control_plane::ControlPlaneError;
    use kallip_agora_common::ids::TagmaId;
    use kallip_agora_common::principal::Principal;
    use kallip_instances::guard::AuthMode;

    let verifier = std::sync::Arc::new(MockVerifier {
        outcomes: std::sync::Mutex::new(vec![
            // Last popped first: reverse program order.
            Err(ControlPlaneError::Backend("agora down".into())),
            Ok(None),
            Ok(Some(Principal::User(
                kallip_agora_common::ids::UserId::from("u1".to_string()),
            ))),
            Ok(Some(Principal::Tagma(TagmaId::from("t1".to_string())))),
            Ok(Some(Principal::Admin)),
        ]),
    });
    let state = AppState {
        backend: kallip_instances::backend::UdsBackend::arc(DaemonClient::new(
            "/nonexistent-kallip-test.sock",
        )),
        auth: AuthMode::Platform(verifier),
        allowed_hosts: vec![],
        cors_origins: String::new(),
    };
    let app = build_router(state);

    // Admin passes the gate (and dies at the daemon proxy: 503 proves the
    // guard let it through).
    let (status, _) = send(&app, "GET", "/api/instances/list", Some("any"), None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    // A valid Tagma identity: 403.
    let (status, body) = send(&app, "GET", "/api/instances/list", Some("any"), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(body.contains("\"forbidden\""), "{body}");

    // A valid User identity: 403.
    let (status, _) = send(&app, "GET", "/api/instances/list", Some("any"), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // An invalid token: 401.
    let (status, body) = send(&app, "GET", "/api/instances/list", Some("any"), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");

    // Agora unreachable: fail closed.
    let (status, body) = send(&app, "GET", "/api/instances/list", Some("any"), None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert!(body.contains("\"auth_backend_unavailable\""), "{body}");
}

#[test]
fn refuses_to_start_unauthenticated_on_non_loopback() {
    // The fail-safe rule: non-loopback bind without either auth mode.
    let config = kallip_instances::Config {
        addr: "0.0.0.0:7300".into(),
        daemon_socket: None,
        token: None,
        backend: "daemon".into(),
        agora_internal_url: None,
        agora_internal_token: None,
        allowed_hosts_raw: String::new(),
        cors_origins: String::new(),
    };
    let error = kallip_instances::resolve_auth(&config, &config.addr).expect_err("must refuse");
    assert!(error.to_string().contains("refusing to start"), "{error}");
}

#[test]
fn open_mode_allowed_on_loopback() {
    let config = kallip_instances::Config {
        addr: "127.0.0.1:7300".into(),
        daemon_socket: None,
        token: None,
        backend: "daemon".into(),
        agora_internal_url: None,
        agora_internal_token: None,
        allowed_hosts_raw: String::new(),
        cors_origins: String::new(),
    };
    assert!(matches!(
        kallip_instances::resolve_auth(&config, &config.addr).expect("resolve"),
        kallip_instances::guard::AuthMode::Open
    ));
}
#[test]
fn half_configured_agora_url_refuses_to_start() {
    // A URL without the internal token must not silently fall through
    // to open mode on a loopback bind.
    let config = kallip_instances::Config {
        addr: "127.0.0.1:7300".into(),
        daemon_socket: None,
        token: None,
        backend: "daemon".into(),
        agora_internal_url: Some("http://127.0.0.1:7100".into()),
        agora_internal_token: None,
        allowed_hosts_raw: String::new(),
        cors_origins: String::new(),
    };
    let error = kallip_instances::resolve_auth(&config, &config.addr).expect_err("must refuse");
    assert!(
        error
            .to_string()
            .contains("KALLIP_INSTANCES_AGORA_INTERNAL_TOKEN"),
        "{error}"
    );
}

#[test]
fn half_configured_agora_token_refuses_to_start() {
    let config = kallip_instances::Config {
        addr: "127.0.0.1:7300".into(),
        daemon_socket: None,
        token: None,
        backend: "daemon".into(),
        agora_internal_url: None,
        agora_internal_token: Some("internal-secret".into()),
        allowed_hosts_raw: String::new(),
        cors_origins: String::new(),
    };
    let error = kallip_instances::resolve_auth(&config, &config.addr).expect_err("must refuse");
    assert!(
        error.to_string().contains("KALLIP_INSTANCES_AGORA_URL"),
        "{error}"
    );
}

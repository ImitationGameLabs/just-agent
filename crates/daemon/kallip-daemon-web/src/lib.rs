//! Local web management proxy for the kallip daemon.
//!
//! Serves the web UI's static build (when configured) and proxies the four
//! management verbs from `/api/daemon/*` to the daemon's UDS socket. The
//! daemon itself never grows an HTTP or token surface; this crate is the
//! only networked door, guarded by a bearer token and a Host-header check.

pub mod api;
pub mod config;
pub mod error;
pub mod guard;

use std::path::Path;

use axum::Router;
use axum::http::StatusCode;
use axum::middleware::{from_fn, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use tower_http::services::{ServeDir, ServeFile};

pub use config::Config;
pub use guard::AppState;

/// Assemble the full router: the API under token auth, static files (when
/// configured) outside it, and the Host guard over everything.
pub fn build_router(state: AppState, static_dir: Option<&Path>) -> Router {
    let api = api::api_routes().layer(from_fn_with_state(state.clone(), guard::token_guard));
    let mut app = Router::new()
        .nest("/api/daemon", api)
        .layer(from_fn(guard::host_guard));

    match static_dir {
        // The fallback serves the SPA: known files straight from disk,
        // unknown paths rewritten to index.html so client-side routes
        // survive a hard refresh.
        Some(dir) => {
            let spa = ServeDir::new(dir).fallback(ServeFile::new(dir.join("index.html")));
            app = app.fallback_service(spa);
        }
        // API-only mode (dev: vite serves the frontend).
        None => app = app.fallback(not_found),
    }
    app.with_state(state)
}

/// API-only mode: anything off the API is a plain 404.
async fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        axum::Json(error::ApiFault {
            code: "not_found",
            message: "no such path; the API lives under /api/daemon".to_string(),
        }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use kallip_daemon_client::DaemonClient;

    fn test_state() -> AppState {
        AppState {
            // A socket that never exists: API calls resolve to 503
            // daemon_unreachable, proving the proxy layer is wired.
            client: DaemonClient::new("/nonexistent-kallip-test.sock"),
            token: "test-token".into(),
        }
    }

    async fn body_string(response: axum::response::Response) -> String {
        let bytes = response.into_body().collect().await.expect("read body");
        String::from_utf8(bytes.to_bytes().to_vec()).expect("utf8")
    }

    #[tokio::test]
    async fn api_requires_a_token() {
        let app = build_router(test_state(), None);
        let response = app
            .oneshot(
                Request::get("/api/daemon/list")
                    .header("host", "127.0.0.1:7300")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = body_string(response).await;
        assert!(body.contains("\"unauthorized\""), "{body}");
    }

    #[tokio::test]
    async fn api_rejects_a_wrong_token() {
        let app = build_router(test_state(), None);
        let response = app
            .oneshot(
                Request::get("/api/daemon/list")
                    .header("host", "127.0.0.1:7300")
                    .header("authorization", "Bearer wrong")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn unreachable_daemon_maps_to_503() {
        let app = build_router(test_state(), None);
        let response = app
            .oneshot(
                Request::get("/api/daemon/list")
                    .header("host", "127.0.0.1:7300")
                    .header("authorization", "Bearer test-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = body_string(response).await;
        assert!(body.contains("\"daemon_unreachable\""), "{body}");
    }

    #[tokio::test]
    async fn foreign_host_is_forbidden_even_with_token() {
        let app = build_router(test_state(), None);
        let response = app
            .oneshot(
                Request::get("/api/daemon/list")
                    .header("host", "evil.example")
                    .header("authorization", "Bearer test-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = body_string(response).await;
        assert!(body.contains("\"host_forbidden\""), "{body}");
    }

    #[tokio::test]
    async fn malformed_json_body_is_bad_request() {
        let app = build_router(test_state(), None);
        let response = app
            .oneshot(
                Request::post("/api/daemon/spawn")
                    .header("host", "127.0.0.1:7300")
                    .header("authorization", "Bearer test-token")
                    .header("content-type", "application/json")
                    .body(Body::from("{not json"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = body_string(response).await;
        assert!(body.contains("\"bad_request\""), "{body}");
    }

    #[tokio::test]
    async fn api_only_mode_404s_off_api_paths() {
        let app = build_router(test_state(), None);
        let response = app
            .oneshot(
                Request::get("/")
                    .header("host", "127.0.0.1:7300")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn static_mode_serves_index_and_falls_back_for_spa_routes() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("index.html"), "<html>kallip</html>").expect("write index");
        let app = build_router(test_state(), Some(dir.path()));
        for path in ["/", "/some/client/route"] {
            let response = app
                .clone()
                .oneshot(
                    Request::get(path)
                        .header("host", "127.0.0.1:7300")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "GET {path}");
            let body = body_string(response).await;
            assert!(body.contains("kallip"), "GET {path}: {body}");
        }
    }

    use tower::ServiceExt;
}

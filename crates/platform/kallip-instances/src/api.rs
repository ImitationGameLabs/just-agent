//! The four management routes. Each is a thin translation: HTTP request →
//! one UDS exchange via `DaemonClient` → the unwrapped payload as plain
//! JSON (the wire's `v`/`kind`/`status` tags stay behind the proxy).

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Query, State};
use axum::http::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use axum::http::{HeaderName, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use kallip_daemon_client::ClientError;
use kallip_daemon_common::wire::{InstanceInfo, OkPayload, RequestBody, ResponseBody};
use serde::Deserialize;

use crate::error::{daemon_err, proxy_err};
use crate::guard::AppState;
use tower_http::cors::{AllowOrigin, CorsLayer};

#[derive(Debug, Deserialize)]
pub struct SpawnRequest {
    pub slug: String,
    pub workspace: String,
    #[serde(default)]
    pub env: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct StopRequest {
    pub slug: String,
}

#[derive(Debug, Deserialize)]
pub struct HealthQuery {
    pub slug: Option<String>,
}

/// The `/api/instances` sub-router. The token guard is applied by the caller
/// Build a CORS layer from a comma-separated allowlist. Mirrors the
/// agora/lesche `cors_layer` (credentials-aware, explicit method list,
/// never a wildcard origin). The tagma has a separate permissive variant
/// -- do NOT copy that one; this is the credentials-aware variant the
/// browser app needs.
pub fn cors_layer(origins: &str) -> CorsLayer {
    let allowed: Vec<HeaderValue> = origins
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect();
    let origin = if allowed.is_empty() {
        AllowOrigin::list(Vec::new())
    } else {
        AllowOrigin::list(allowed)
    };
    CorsLayer::new()
        .allow_origin(origin)
        // Methods must be an explicit list, NOT `Any`: the Fetch spec
        // forbids `Access-Control-Allow-Credentials: true` together with
        // a wildcard (`Allow-Methods: *`), and tower-http panics at
        // layer construction if they're combined. The management API
        // speaks exactly these two verbs.
        .allow_methods([Method::GET, Method::POST])
        // Allow credentialed cross-origin requests so the web app --
        // served from a different origin than this service -- can send
        // its bearer header after a passing preflight. Safe because
        // every wildcard-forbidden field is concrete: the origin
        // allowlist is `AllowOrigin::list` (never `Any`) and the methods
        // are enumerated above. A misconfigured
        // `KALLIP_INSTANCES_CORS_ORIGINS=*` therefore yields an empty
        // allowlist (no cross-origin allowed) rather than an open hole.
        .allow_credentials(true)
        // `Authorization` is excluded from the `*` wildcard by the Fetch
        // spec, so list the request headers we actually send explicitly.
        .allow_headers([
            AUTHORIZATION,
            CONTENT_TYPE,
            ACCEPT,
            HeaderName::from_static("x-requested-with"),
        ])
}

/// (`build_router`), not here, so tests can hit the handlers directly.
pub fn api_routes() -> Router<AppState> {
    Router::new()
        .route("/spawn", post(spawn))
        .route("/stop", post(stop))
        .route("/list", get(list))
        .route("/health", get(health))
}

async fn spawn(
    State(state): State<AppState>,
    payload: Result<Json<SpawnRequest>, JsonRejection>,
) -> Response {
    let Json(SpawnRequest {
        slug,
        workspace,
        env,
    }) = match payload {
        Ok(Json(body)) => Json(body),
        Err(rejection) => return bad_body(rejection),
    };
    let response = state
        .client
        .call(RequestBody::Spawn {
            slug,
            workspace,
            env,
        })
        .await;
    unwrap(response)
}

async fn stop(
    State(state): State<AppState>,
    payload: Result<Json<StopRequest>, JsonRejection>,
) -> Response {
    let Json(StopRequest { slug }) = match payload {
        Ok(Json(body)) => Json(body),
        Err(rejection) => return bad_body(rejection),
    };
    let response = state.client.call(RequestBody::Stop { slug }).await;
    unwrap(response)
}

async fn list(State(state): State<AppState>) -> Response {
    let response = state.client.call(RequestBody::List).await;
    unwrap(response)
}

async fn health(
    State(state): State<AppState>,
    query: Result<Query<HealthQuery>, QueryRejection>,
) -> Response {
    let Query(HealthQuery { slug }) = match query {
        Ok(query) => query,
        Err(rejection) => return bad_body(rejection),
    };
    let response = state.client.call(RequestBody::Health { slug }).await;
    unwrap(response)
}

/// Translate one UDS exchange into an HTTP response: Ok → the unwrapped
/// payload's plain JSON; Err → the mapped status + `{code, message}`.
fn unwrap(response: Result<kallip_daemon_common::wire::Response, ClientError>) -> Response {
    match response {
        Ok(wire) => match wire.body {
            ResponseBody::Ok { payload } => ok_payload(payload).into_response(),
            ResponseBody::Err { code, message } => daemon_err(code, message),
        },
        Err(error) => proxy_err(error),
    }
}

/// Strip the wire tags from a success payload so the browser sees plain
/// JSON (`spawn` → `{slug, pid, port}`, `list` → `{instances: [...]}`, and
/// so on).
fn ok_payload(payload: OkPayload) -> Response {
    match payload {
        OkPayload::Spawn { slug, pid, port } => Json(Spawned { slug, pid, port }).into_response(),
        OkPayload::Stop { slug } => Json(Stopped { slug }).into_response(),
        OkPayload::List { instances } => Json(InstanceList { instances }).into_response(),
        OkPayload::Health { report } => Json(report).into_response(),
    }
}

/// A JSON body that failed to parse becomes 400 `bad_request` (not axum's
/// default 415/422/500 text): the daemon's own grammar for a malformed
/// request, so clients see one error shape.
fn bad_body(rejection: impl std::fmt::Display) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(crate::error::ApiFault {
            code: "bad_request",
            message: format!("request body rejected: {rejection}"),
        }),
    )
        .into_response()
}

#[derive(Debug, serde::Serialize)]
pub struct Spawned {
    pub slug: String,
    pid: u32,
    port: u16,
}

#[derive(Debug, serde::Serialize)]
pub struct Stopped {
    pub slug: String,
}

#[derive(Debug, serde::Serialize)]
pub struct InstanceList {
    pub instances: Vec<InstanceInfo>,
}

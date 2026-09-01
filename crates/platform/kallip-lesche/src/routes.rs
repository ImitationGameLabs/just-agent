//! Data-plane route mounting.

mod conversations;
mod direct;
mod events;
mod internal;
mod room_management;
mod rooms;
mod signal;
mod status;
mod tunnel;

#[cfg(test)]
pub(crate) mod test_support;

use axum::Router;
use axum::http::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use axum::http::{HeaderName, HeaderValue, Method};
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::state::SharedConvState;

/// Data-plane routes, state-injected (`Router<()>`): `/conversations*`,
/// `/me/events`, `/tagmata/{id}/status`, `/tagmata/{id}/signal`, and `/tunnel`.
pub fn router(
    state: SharedConvState,
    internal_token_hash: Option<kallip_common::authtoken::TokenHash>,
) -> Router<()> {
    let mut app = Router::new()
        .merge(conversations::router().with_state(state.clone()))
        .merge(rooms::router().with_state(state.clone()))
        .merge(direct::router().with_state(state.clone()))
        .merge(room_management::router().with_state(state.clone()))
        .merge(events::router().with_state(state.clone()))
        .merge(signal::router().with_state(state.clone()))
        .merge(status::router().with_state(state.clone()))
        .merge(tunnel::router().with_state(state.clone()));

    // The service-to-service `/internal/*` surface: mounted only when the
    // shared secret is configured (same discipline as the agora's internal
    // nest; the files service pushes FileDelivered events here).
    if let Some(hash) = internal_token_hash {
        let internal = internal::router(state.clone()).layer(axum::middleware::from_fn_with_state(
            hash,
            crate::middleware::internal_guard,
        ));
        app = app.nest("/internal", internal);
    }
    app
}
/// Build a CORS layer from a comma-separated allowlist. Mirrors the agora's
/// `cors_layer` (credentials-aware, explicit method list, never a wildcard
/// origin). The tagma has a separate permissive `cors_layer` -- do NOT copy
/// that one; this is the credentials-aware variant the browser app needs.
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
        // Methods must be an explicit list, NOT `Any`: the Fetch spec forbids
        // `Access-Control-Allow-Credentials: true` together with a wildcard
        // (`Allow-Methods: *`), and tower-http panics at layer construction if
        // they're combined.
        // PUT is the room read-cursor write (the web app's unread sync);
        // without it the browser rejects the preflight and the cursor write
        // never lands.
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
        ])
        // Allow credentialed (cookie-bearing) cross-origin requests so the web
        // app -- served from a different origin than the lesche -- can send the
        // `kallip_session` cookie with `credentials: "include"`. Safe because
        // every wildcard-forbidden field is concrete: the origin allowlist is
        // `AllowOrigin::list` (never `Any`) and the methods are enumerated
        // above. A misconfigured `KALLIP_LESCHE_CORS_ORIGINS=*` therefore yields
        // an empty allowlist (no cross-origin allowed) rather than an open hole.
        .allow_credentials(true)
        // `Authorization` is excluded from the `*` wildcard by the Fetch spec,
        // so list the request headers we actually send explicitly. The CSRF
        // marker (`X-Requested-With`) is a custom header the browser only sends
        // same-origin / after a passing preflight, so it must be allowed here
        // for the preflight to succeed.
        .allow_headers([
            AUTHORIZATION,
            CONTENT_TYPE,
            ACCEPT,
            HeaderName::from_static("x-requested-with"),
        ])
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::header::{ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_REQUEST_METHOD, ORIGIN};
    use axum::http::{Method, Request, StatusCode};
    use axum::routing::get;
    use tower::ServiceExt;

    use super::cors_layer;
    use axum::Router;

    /// Same pin as the agora's: the advertised preflight set must equal the
    /// table exactly, so dropping a method (the omission that broke the
    /// agora's provider vault) fails here instead of in a live session.
    /// The advertised PATCH has no patch route yet; removing it is deferred
    /// to a future CORS cleanup on purpose.
    #[tokio::test]
    async fn preflight_advertises_exactly_the_route_methods() {
        let app = Router::new()
            .route("/ping", get(|| async { "ok" }))
            .layer(cors_layer("https://app.example"));
        let request = Request::builder()
            .method(Method::OPTIONS)
            .header(ORIGIN, "https://app.example")
            .header(ACCESS_CONTROL_REQUEST_METHOD, "PUT")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let advertised = response
            .headers()
            .get(ACCESS_CONTROL_ALLOW_METHODS)
            .expect("allowed preflight advertises the method list")
            .to_str()
            .unwrap();
        let mut advertised: Vec<&str> = advertised.split(',').map(str::trim).collect();
        advertised.sort_unstable();
        assert_eq!(advertised, ["DELETE", "GET", "PATCH", "POST", "PUT"]);
    }

    /// Assembly smoke test: the full router build (every sub-router
    /// merged, the internal nest conditionally mounted) must not panic.
    /// axum 0.8 panics on a same-path merge conflict at construction,
    /// and only the binary's startup path builds the whole router -- the
    /// per-sub-router tests never exercise this (the quality CRITICAL
    /// from the F0 review: a duplicated merge line crashed startup while
    /// the test suite stayed green).
    #[test]
    fn full_router_assembly_does_not_panic() {
        let (state, _control) =
            crate::test_support::make_state(60, std::time::Duration::from_secs(10));
        // Both mount states: no internal surface, and with it.
        let _ = super::router(state.clone(), None);
        let _ = super::router(
            state,
            Some(kallip_common::authtoken::TokenHash::of("secret")),
        );
    }
}

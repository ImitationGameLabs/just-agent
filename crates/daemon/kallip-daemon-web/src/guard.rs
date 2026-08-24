//! Request guards: bearer-token auth for the API surface and a Host-header
//! check on everything.
//!
//! The token check is constant-time (`subtle`) because LAN deployments put
//! this server on a network where timing a byte-at-a-time comparison is a
//! plausible remote attack. The Host check blocks DNS rebinding: an attacker
//! page rebinding to this port must send its own domain as Host, so only IP
//! literals and localhost are allowed through.

use axum::extract::State;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use subtle::ConstantTimeEq;

use crate::error::fault;

/// Shared handler state: the UDS client plus the configured token.
#[derive(Debug, Clone)]
pub struct AppState {
    pub client: kallip_daemon_client::DaemonClient,
    pub token: String,
}

/// Bearer-token guard for `/api/daemon/*`. Static assets are served outside
/// this layer: the page loads first, then its API calls carry the token.
pub async fn token_guard(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let presented = match kallip_common::auth_header::extract_bearer_token(&headers) {
        Ok(token) => token,
        // The shared helper reports both "missing" and "malformed"; either
        // way the caller holds no usable credential.
        Err(_) => {
            return fault(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "a bearer token is required (Authorization: Bearer <token>)",
            );
        }
    };
    // Constant-time compare, length-checked first: ct_eq only guarantees
    // timing safety for equal lengths, and the length itself is not secret
    // (token format is fixed).
    let expected = state.token.as_bytes();
    let presented = presented.as_bytes();
    let same = expected.len() == presented.len() && expected.ct_eq(presented).into();
    if !same {
        return fault(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "invalid bearer token",
        );
    }
    next.run(request).await
}

/// Host-header guard, applied to the whole router (API and static). A Host
/// that is neither an IP literal nor `localhost` gets 403 — DNS-rebinding
/// attacks necessarily carry an attacker-controlled hostname here.
pub async fn host_guard(request: Request<axum::body::Body>, next: Next) -> Response {
    let host = request
        .headers()
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok());
    let ok = host.is_some_and(host_is_allowed);
    if !ok {
        return fault(
            StatusCode::FORBIDDEN,
            "host_forbidden",
            "the Host header must be an IP address or localhost",
        );
    }
    next.run(request).await
}

/// One Host value (possibly `host:port` or `[v6]:port`) is allowed when it
/// names an IP literal or localhost.
fn host_is_allowed(host: &str) -> bool {
    // Split off one port; bare IPv6 literals keep their colons (the port
    // side must be all digits and non-empty, which a hex IPv6 tail is not).
    let authority = match host.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => h,
        _ => host,
    };
    if authority.eq_ignore_ascii_case("localhost") {
        return true;
    }
    let authority = authority.trim_start_matches('[').trim_end_matches(']');
    authority.parse::<std::net::IpAddr>().is_ok()
        // A bare unbracketed IPv6 ("::1") mis-splits above (the tail is
        // digits); parsing the original value covers that rare form.
        || host.parse::<std::net::IpAddr>().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_ip_literals_and_localhost() {
        for good in [
            "127.0.0.1",
            "127.0.0.1:7300",
            "192.168.1.4:7300",
            "[::1]:7300",
            "[2001:db8::1]",
            "::1",
            "localhost",
            "LOCALHOST:7300",
        ] {
            assert!(host_is_allowed(good), "{good} should be allowed");
        }
    }

    #[test]
    fn rejects_domains_and_garbage() {
        for bad in [
            "evil.example",
            "evil.example:80",
            "127.0.0.1.evil.example",
            "localhost.evil.example",
            "",
            "not a host",
        ] {
            assert!(!host_is_allowed(bad), "{bad} should be rejected");
        }
    }
}

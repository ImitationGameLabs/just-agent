//! Thin HTTP client for the agora's service-to-service verify-bearer call.
//!
//! The lesche reaches the agora through `HttpControlPlane` (private to that
//! crate, and implementing the full six-method `ControlPlane` trait); this
//! proxy needs exactly one call, so it carries a one-method verifier of its
//! own rather than pulling the relay crate in or refactoring a shared
//! client out (a tracked follow-up if a third consumer appears).

use kallip_agora_common::control_plane::ControlPlaneError;
use kallip_agora_common::internal_api::{VerifyBearerRequest, VerifyBearerResponse};
use kallip_agora_common::principal::Principal;

/// Per-call timeout: a tiny JSON round trip against a local agora; 10s is a
/// generous backstop, matching the lesche's client.
const INTERNAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// What the auth guard needs from an auth backend: verify one bearer token.
#[async_trait::async_trait]
pub trait BearerVerifier: Send + Sync {
    /// `Err` = the backend could not be reached or answered outside the
    /// contract (the guard fails closed on this); `Ok(None)` = the token is
    /// simply not valid.
    async fn verify_bearer(&self, token: &str) -> Result<Option<Principal>, ControlPlaneError>;
}

/// Agora-backed [`BearerVerifier`]: one POST per call to
/// `/internal/verify-bearer`, guarded by the shared internal secret.
#[derive(Clone)]
pub struct AgoraVerifier {
    /// Agora internal root (e.g. `http://127.0.0.1:7100`).
    base_url: String,
    /// Shared secret matching the agora's `KALLIP_AGORA_INTERNAL_TOKEN`.
    internal_token: String,
    http: reqwest::Client,
}

impl AgoraVerifier {
    pub fn new(base_url: String, internal_token: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(INTERNAL_TIMEOUT)
            .build()
            .expect("build reqwest client");
        Self {
            base_url,
            internal_token,
            http,
        }
    }
}

#[async_trait::async_trait]
impl BearerVerifier for AgoraVerifier {
    async fn verify_bearer(&self, token: &str) -> Result<Option<Principal>, ControlPlaneError> {
        let response = self
            .http
            .post(format!("{}/internal/verify-bearer", self.base_url))
            .bearer_auth(&self.internal_token)
            .json(&VerifyBearerRequest {
                token: token.to_string(),
            })
            .send()
            .await
            .map_err(|e| ControlPlaneError::Backend(e.to_string()))?;
        match response.status().as_u16() {
            200 => response
                .json::<VerifyBearerResponse>()
                .await
                .map(|r| Some(Principal::from(r.principal)))
                .map_err(|e| ControlPlaneError::Backend(e.to_string())),
            404 => Ok(None),
            status => Err(ControlPlaneError::Backend(format!(
                "agora /internal/verify-bearer returned HTTP {status}"
            ))),
        }
    }
}

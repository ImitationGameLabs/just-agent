//! Two-layer auth: Authentication (who are you) then Authorization (can you do this).

use axum::extract::FromRequestParts;

use crate::state::{AgentId, SharedState};
use just_agent_common::protocol::ApiError;

/// Resolved identity from the Authorization header.
#[derive(Debug, Clone)]
pub enum Identity {
    /// Caller authenticated as the operator (superuser).
    Operator,
    /// Caller authenticated as a specific agent.
    Agent { id: AgentId },
}

/// axum extractor that resolves a Bearer token to an [`Identity`].
///
/// Layer 1 (Authentication): parses the `Authorization: Bearer <token>` header,
/// matches against `operator_token` first, then checks the registry token index.
#[derive(Debug, Clone)]
pub struct AuthIdentity(Identity);

impl AuthIdentity {
    /// Access the resolved [`Identity`].
    pub fn identity(&self) -> &Identity {
        &self.0
    }

    /// Construct an [`AuthIdentity`] for testing.
    #[cfg(test)]
    pub(crate) fn test_new(identity: Identity) -> Self {
        Self(identity)
    }
}

impl FromRequestParts<SharedState> for AuthIdentity {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &SharedState,
    ) -> Result<Self, Self::Rejection> {
        let token = extract_token(&parts.headers)?;

        // NOTE: Non-constant-time comparison is acceptable: agents authenticate
        // over localhost, and operator access over open networks will require
        // HTTPS. In neither case is timing a practical attack vector.
        if state.operator_token == token {
            return Ok(AuthIdentity(Identity::Operator));
        }

        let registry = state.registry.read().await;
        if let Some(id) = registry.get_agent_id_by_token(token) {
            return Ok(AuthIdentity(Identity::Agent { id: id.clone() }));
        }

        Err(ApiError::unauthorized("invalid agent token"))
    }
}

// ---------------------------------------------------------------------------
// Layer 2: Authorization helpers
// ---------------------------------------------------------------------------

/// Only the operator may proceed. Used for root agent creation and
/// daemon-wide resource management (e.g. token budget).
pub fn require_operator(identity: &Identity) -> Result<(), ApiError> {
    match identity {
        Identity::Operator => Ok(()),
        Identity::Agent { .. } => Err(ApiError::forbidden("operator access required")),
    }
}

/// Extract bearer token from the Authorization header.
fn extract_token(headers: &axum::http::HeaderMap) -> Result<&str, ApiError> {
    let value = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| ApiError::unauthorized("authentication required"))?;
    let token = value
        .strip_prefix("Bearer ")
        .ok_or_else(|| ApiError::unauthorized("invalid Authorization scheme, expected Bearer"))?;
    if token.is_empty() {
        return Err(ApiError::unauthorized("empty bearer token"));
    }
    Ok(token)
}

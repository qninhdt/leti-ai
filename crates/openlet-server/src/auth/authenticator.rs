//! Inbound `Authenticator` seam.
//!
//! openlet is strictly zero-trust: identity is never taken from an
//! upstream-injected header — it must be derived from a verified token
//! inside (or directly in front of) the service. openlet-ai ships only
//! the trait + a local dev default; the cloud JWKS verifier lives in the
//! openlet repo and plugs in here.

use async_trait::async_trait;
use axum::http::HeaderMap;

use super::principal::AuthPrincipal;

/// Why authentication failed. Maps to `401 Unauthorized` at the layer —
/// the message is logged but never leaked to the client verbatim (avoids
/// oracle-ing token internals).
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// No credential presented (missing/empty Authorization header).
    #[error("missing credential")]
    MissingCredential,
    /// Credential present but invalid (bad signature, expired, wrong
    /// principal class). Cloud verifier surfaces specifics here.
    #[error("invalid credential: {0}")]
    InvalidCredential(String),
}

/// Inbound authentication seam. Runs once per request before the
/// workspace layer; its `AuthPrincipal` output is the ONLY identity the
/// rest of the stack trusts.
#[async_trait]
pub trait Authenticator: Send + Sync + 'static {
    /// Verify the request's credentials and produce the caller principal.
    /// Implementations MUST NOT trust pre-set identity headers — derive
    /// identity from a verifiable credential (or, for the local dev
    /// default, issue a fixed principal).
    async fn authenticate(&self, headers: &HeaderMap) -> Result<AuthPrincipal, AuthError>;
}

/// Local-binary default: admits a single configured dev principal on
/// every request without any token. This is the `./openlet-ai` posture —
/// loopback-only, no auth server. It is **dev-only**: boot refuses to
/// pair it with a non-loopback bind or the `cloud` runtime profile
/// (fail-closed; see `main.rs`).
#[derive(Debug, Clone)]
pub struct LocalDevAuthenticator {
    principal: AuthPrincipal,
}

impl LocalDevAuthenticator {
    /// Build with an explicit dev caller id.
    #[must_use]
    pub fn new(caller_id: impl Into<String>) -> Self {
        Self {
            principal: AuthPrincipal::user(caller_id),
        }
    }
}

impl Default for LocalDevAuthenticator {
    fn default() -> Self {
        Self::new("local-dev")
    }
}

#[async_trait]
impl Authenticator for LocalDevAuthenticator {
    async fn authenticate(&self, _headers: &HeaderMap) -> Result<AuthPrincipal, AuthError> {
        // Local posture: every request is the same trusted dev principal.
        Ok(self.principal.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::principal::PrincipalType;

    #[tokio::test]
    async fn local_dev_admits_a_fixed_principal() {
        let auth = LocalDevAuthenticator::default();
        let p = auth.authenticate(&HeaderMap::new()).await.unwrap();
        assert_eq!(p.caller_id, "local-dev");
        assert_eq!(p.principal_type, PrincipalType::User);
    }

    #[tokio::test]
    async fn local_dev_honors_explicit_caller_id() {
        let auth = LocalDevAuthenticator::new("alice");
        let p = auth.authenticate(&HeaderMap::new()).await.unwrap();
        assert_eq!(p.caller_id, "alice");
    }
}

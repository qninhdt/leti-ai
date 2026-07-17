//! Inbound `Authenticator` seam.
//!
//! leti is strictly zero-trust: identity is never taken from an
//! upstream-injected header — it must be derived from a verified token
//! inside (or directly in front of) the service. leti-ai ships only
//! the trait + a local dev default; the cloud JWKS verifier lives in the
//! leti repo and plugs in here.

use std::sync::Arc;

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

    /// Whether this authenticator admits requests without verifying a
    /// real credential. The local dev default returns `true`; any cloud
    /// verifier returns `false`. Boot uses this to refuse a dev
    /// authenticator on a non-loopback bind (fail-closed).
    fn is_dev(&self) -> bool {
        false
    }
}

/// Local-binary default: admits a single configured dev principal on
/// every request without any token. This is the `./leti-ai` posture —
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

    fn is_dev(&self) -> bool {
        true
    }
}

/// Resolve the authenticator for the reference loopback binary. Host
/// applications should construct their own authenticator and pass it to
/// [`crate::router::RouterBuilder::build_with_auth`].
pub fn authenticator_for_profile() -> anyhow::Result<Arc<dyn Authenticator>> {
    Ok(Arc::new(LocalDevAuthenticator::default()))
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

    #[test]
    fn local_dev_is_dev_true() {
        assert!(LocalDevAuthenticator::default().is_dev());
    }

    #[test]
    fn reference_resolves_dev_authenticator() {
        let auth = authenticator_for_profile().expect("reference resolves an authenticator");
        assert!(auth.is_dev());
    }
}

//! Canonical identity types for the inbound auth seam.
//!
//! ONE `AuthPrincipal` is shared by the auth layer, the workspace-routing
//! gate, and the question route — its type identity is what the gate
//! looks up in request extensions, so a second same-named type would
//! silently fail the `TypeId` match and 401 every request. Cloud
//! integrators construct this from their JWKS-verified token; the local
//! binary's `LocalDevAuthenticator` constructs a fixed dev principal.

/// Who is making the request. Carried through the auth layer into request
/// extensions; the workspace resolver consults it for ownership checks.
///
/// `principal_type` is carried now even though no check consumes every
/// variant yet — it is the seam a cloud authenticator populates from the
/// verified token's principal class (user vs. agent vs. service account),
/// and the ownership path will branch on it as the cloud contract firms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthPrincipal {
    /// Stable id of the caller (user id, agent id, or SA id depending on
    /// `principal_type`). Opaque string — openlet-ai never parses it.
    pub caller_id: String,
    pub principal_type: PrincipalType,
}

impl AuthPrincipal {
    /// Construct a user-typed principal (the common inbound case).
    #[must_use]
    pub fn user(caller_id: impl Into<String>) -> Self {
        Self {
            caller_id: caller_id.into(),
            principal_type: PrincipalType::User,
        }
    }
}

/// Class of caller. A cloud authenticator sets this from the verified
/// token; the local dev authenticator always issues `User`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrincipalType {
    /// An end user calling through the product surface.
    User,
    /// Another agent calling this agent (e.g. leti→agent chat). Reserved
    /// for the cloud contract; no local path issues it yet.
    Agent,
    /// A service account (machine-to-machine). Reserved for the cloud
    /// contract.
    Service,
}

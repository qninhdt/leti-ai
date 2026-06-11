//! Inbound auth + outbound credential seams.
//!
//! Two pluggable traits the cloud repo plugs into, each with a local
//! default so `./openlet-ai` works with no auth server:
//! - [`Authenticator`] (inbound): verifies the request and yields the
//!   canonical [`AuthPrincipal`]. Local default: [`LocalDevAuthenticator`].
//! - [`CredentialProvider`] (outbound): mints the SA credential an agent's
//!   outbound calls carry. Local default: [`NoopCredentialProvider`].
//!
//! [`AuthLayer`] mounts the authenticator as a tower layer; it runs before
//! the workspace-routing layer so the `AuthPrincipal` is in extensions for
//! the workspace gate to find.

pub mod authenticator;
pub mod credential;
pub mod layer;
pub mod principal;

pub use authenticator::{
    AuthError, Authenticator, LocalDevAuthenticator, RuntimeProfile, authenticator_for_profile,
};
pub use credential::{
    CredentialError, CredentialProvider, NoopCredentialProvider, OutboundCredential,
};
pub use layer::AuthLayer;
pub use principal::{AgentWorkspace, AuthPrincipal, PrincipalType};

//! Library surface for `openlet-server` — exports the symbols
//! downstream integrators need (AppState, AppStateBuilder, RouterBuilder,
//! routes) without forcing them to duplicate boot wiring.

pub mod app_state;
pub mod app_state_builder;
pub mod audit;
pub mod auth;
pub mod cli;
pub mod core_api_impl;
pub mod diagnostics;
pub mod error;
pub mod events;
pub(crate) mod mention;
pub mod middleware;
pub mod notif_bucket;
pub mod openapi;
pub mod router;
pub mod routes;
pub mod subagent_driver;
pub mod subagent_spawner;
pub mod turn_driver;
pub mod workspace_resolver;

pub use app_state::{AgentResources, AppState, TurnHandle};
pub use app_state_builder::{AppStateBuilder, AppStateBuilderError};
pub use auth::{
    AgentWorkspace, AuthError, AuthLayer, AuthPrincipal, Authenticator, CredentialError,
    CredentialProvider, LocalDevAuthenticator, NoopCredentialProvider, OutboundCredential,
    PrincipalType,
};
pub use error::AppError;
pub use middleware::{WORKSPACE_HEADER, WorkspaceRoutingGuard, WorkspaceRoutingLayer};
pub use router::RouterBuilder;
pub use subagent_spawner::RuntimeSubagentSpawner;
pub use workspace_resolver::{
    StaticWorkspaceResolver, WorkspaceError, WorkspaceResolver, workspace_data_root,
};

/// Re-export of `router::build` under a shorter name for tests + the
/// reference binary. Equivalent to `RouterBuilder::default().build(state)`.
pub use router::build as build_router;

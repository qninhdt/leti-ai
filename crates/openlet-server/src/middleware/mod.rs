//! HTTP middleware layers — workspace routing, request shaping.

pub mod workspace_routing;

pub use workspace_routing::{
    AuthPrincipal, WORKSPACE_HEADER, WorkspaceRoutingGuard, WorkspaceRoutingLayer,
};

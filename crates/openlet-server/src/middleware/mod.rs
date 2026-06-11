//! HTTP middleware layers — workspace routing, request shaping.

pub mod workspace_routing;

pub use workspace_routing::{WORKSPACE_HEADER, WorkspaceRoutingGuard, WorkspaceRoutingLayer};

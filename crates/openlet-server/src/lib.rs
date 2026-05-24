//! Library surface for `openlet-server` — exports the symbols
//! downstream integrators need (AppState, AppStateBuilder, RouterBuilder,
//! routes) without forcing them to duplicate boot wiring.

pub mod app_state;
pub mod app_state_builder;
pub mod audit;
pub mod cli;
pub mod core_api_impl;
pub mod error;
pub mod openapi;
pub mod router;
pub mod routes;

pub use app_state::{AgentResources, AppState, TurnHandle};
pub use app_state_builder::{AppStateBuilder, AppStateBuilderError};
pub use error::AppError;
pub use router::RouterBuilder;

/// Re-export of `router::build` under a shorter name for tests + the
/// reference binary. Equivalent to `RouterBuilder::default().build(state)`.
pub use router::build as build_router;

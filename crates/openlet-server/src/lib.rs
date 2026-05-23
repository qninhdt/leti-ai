//! Library surface for `openlet-server` — exports the symbols
//! integration tests need (AppState, build_router) without forcing
//! tests to duplicate boot wiring.

pub mod app_state;
pub mod cli;
pub mod error;
pub mod openapi;
pub mod router;
pub mod routes;

pub use app_state::{AgentResources, AppState, TurnHandle};
pub use error::AppError;

/// Re-export of `router::build` under a shorter name for tests.
pub use router::build as build_router;

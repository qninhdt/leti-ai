//! Openlet HTTP DTOs — utoipa-derived schemas shared by server + future SDK.
//!
//! Kept separate from `openlet-core` so domain types stay HTTP-agnostic.

pub mod dto;

pub use dto::HealthDto;

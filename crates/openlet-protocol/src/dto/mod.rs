//! HTTP DTOs — kept separate from `openlet-core` so domain types stay
//! HTTP-agnostic. Each module is one DTO group.

pub mod health;

pub use health::HealthDto;

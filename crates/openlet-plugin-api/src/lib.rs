//! Stable plugin API — the surface plugin authors depend on.
//!
//! Versioned independently from `openlet-core`. The `Plugin` trait + hook
//! signatures + `HookResult` define the entire extension contract.

pub mod context;
pub mod hooks;
pub mod manifest;
pub mod plugin;
pub mod prelude;

pub use context::PluginContext;
pub use hooks::{HookKind, HookResult, Priority};
pub use manifest::{Capability, PluginManifest};
pub use plugin::{Plugin, PluginError};

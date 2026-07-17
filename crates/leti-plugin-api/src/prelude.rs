//! Convenient re-exports for plugin authors.
//!
//! `use leti_plugin_api::prelude::*;` brings in the core trait, manifest,
//! hook types, and context.

pub use crate::context::{CoreApi, PluginContext};
pub use crate::hooks::{HookKind, HookResult, Priority};
pub use crate::manifest::{Capability, PluginManifest};
pub use crate::plugin::{Plugin, PluginError};

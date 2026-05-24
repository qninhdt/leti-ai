//! Re-exports of the hook surface from `openlet-core`.
//!
//! Hook types live in `openlet-core::hooks` so runtime dispatch sites
//! can construct ctx values without a circular dep on this crate.
//! Plugin authors continue to import them via `openlet_plugin_api`.

pub use openlet_core::hooks::{HookKind, HookResult, Priority, io};

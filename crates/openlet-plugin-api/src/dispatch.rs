//! Re-exports of the hook dispatch surface from `openlet-core`.
//!
//! Dispatch types live in `openlet-core::dispatch` so runtime call
//! sites can invoke [`dispatch`] without a circular dep on this crate.
//! Plugin authors continue to import them via `openlet_plugin_api`.

pub use openlet_core::dispatch::{
    DispatchOutcome, FaultKind, HookChains, HookEntry, HookFn, HookFuture, PluginFault, dispatch,
    dispatch_event,
};

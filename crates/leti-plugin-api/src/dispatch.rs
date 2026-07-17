//! Re-exports of the hook dispatch surface from `leti-core`.
//!
//! Dispatch types live in `leti-core::dispatch` so runtime call
//! sites can invoke [`dispatch`] without a circular dep on this crate.
//! Plugin authors continue to import them via `leti_plugin_api`.

pub use leti_core::dispatch::{
    DispatchOutcome, FaultKind, HookChains, HookEntry, HookFn, HookFuture, PluginFault, dispatch,
    dispatch_event,
};

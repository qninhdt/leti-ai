//! Permission domain types — re-exports + `Deferred<T>` resolver helper.
//!
//! Top-level types (`PermissionMode`, `PermissionRule`, `PermissionAction`,
//! `AskId`, `Decision`, `PermissionRequest`, `PermissionCtx`,
//! `AlwaysScope`) live in [`crate::types::permission`]; this module
//! re-exports them for ergonomic `use openlet_core::permission::…` paths
//! and adds the runtime-side `Deferred` future used to await an ask.

mod deferred;

pub use deferred::{Deferred, DeferredSender, deferred_pair};

pub use crate::types::permission::{
    AlwaysScope, AskId, Decision, PermissionAction, PermissionCtx, PermissionMode,
    PermissionRequest, PermissionRule,
};

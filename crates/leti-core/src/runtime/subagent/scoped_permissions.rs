//! Permission delegation for nested subagent sessions.
//!
//! [`ScopedPermissionManager`] wraps a parent [`PermissionManager`] and
//! filters by the child agent's tool allowlist. The chain is walked
//! DYNAMICALLY at every check (not snapshotted at construction) so a
//! mid-turn agent definition swap propagates to descendants — when a
//! grandchild calls `check`, it asks its parent (also a
//! `ScopedPermissionManager`), which asks its own parent, etc., AND-ing
//! every layer's allowlist into the final decision.
//!
//! Allowlist semantics: an empty `child_allowlist` means inherit-all from
//! the parent (no extra filtering at this layer). A non-empty allowlist
//! restricts the child to that tool set; permissions outside the set are
//! denied without consulting the parent.

use std::sync::Arc;

use async_trait::async_trait;

use crate::adapters::permission_manager::PermissionManager;
use crate::error::PermissionError;
use crate::permission::Deferred;
use crate::types::permission::{
    AlwaysScope, AskId, Decision, PermissionAction, PermissionCtx, PermissionRequest,
    PermissionRule,
};
use crate::types::session::SessionId;

/// Permission delegate for a nested subagent session.
///
/// `child_allowlist` is matched against the LEADING tool name of the
/// permission subject (e.g. `"read"` matches `"read:foo.rs"`). Empty list
/// = inherit-all. Reply / ask APIs forward to the parent unchanged so a
/// human reviewer always sees the prompt at the root session.
pub struct ScopedPermissionManager {
    parent: Arc<dyn PermissionManager>,
    child_allowlist: Vec<String>,
}

impl ScopedPermissionManager {
    /// Construct a child manager. `child_allowlist` is the agent's
    /// `tool_allowlist`; pass an empty `Vec` to inherit the parent's
    /// permissions verbatim.
    #[must_use]
    pub fn new(parent: Arc<dyn PermissionManager>, child_allowlist: Vec<String>) -> Self {
        Self {
            parent,
            child_allowlist,
        }
    }

    /// `true` iff this layer permits a request with `permission` (e.g.
    /// `"read:foo.rs"`). Inherit-all returns `true` unconditionally.
    /// Public for property-test introspection.
    #[must_use]
    pub fn allows(&self, permission: &str) -> bool {
        if self.child_allowlist.is_empty() {
            return true;
        }
        let head = permission.split(':').next().unwrap_or(permission);
        self.child_allowlist.iter().any(|t| t == head)
    }
}

#[async_trait]
impl PermissionManager for ScopedPermissionManager {
    async fn check(
        &self,
        ctx: PermissionCtx,
        req: PermissionRequest,
    ) -> Result<Decision, PermissionError> {
        // Dynamic chain walk: this layer's allowlist AND parent's check.
        // Grandchild's parent IS its parent's ScopedPermissionManager,
        // which delegates further up — emergent depth-N evaluation
        // without snapshotting.
        if !self.allows(&req.permission) {
            return Ok(Decision::Deny {
                feedback: Some(format!(
                    "tool not in subagent allowlist: {}",
                    req.permission
                )),
            });
        }
        self.parent.check(ctx, req).await
    }

    async fn reply(&self, ask_id: AskId, decision: Decision) -> Result<(), PermissionError> {
        self.parent.reply(ask_id, decision).await
    }

    async fn cancel_ask(&self, ask_id: AskId) -> Result<(), PermissionError> {
        self.parent.cancel_ask(ask_id).await
    }

    async fn record_always(
        &self,
        scope: AlwaysScope,
        rule: PermissionRule,
    ) -> Result<(), PermissionError> {
        self.parent.record_always(scope, rule).await
    }

    fn take_deferred(&self, ask_id: AskId) -> Option<Deferred<Decision>> {
        self.parent.take_deferred(ask_id)
    }

    fn peek_session_id(&self, ask_id: AskId) -> Option<SessionId> {
        self.parent.peek_session_id(ask_id)
    }

    async fn accept_ask(
        &self,
        ask_id: AskId,
        scope: AlwaysScope,
        action: PermissionAction,
    ) -> Result<(), PermissionError> {
        self.parent.accept_ask(ask_id, scope, action).await
    }
}

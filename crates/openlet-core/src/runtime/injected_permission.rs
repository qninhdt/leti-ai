//! Fail-closed permission shim for autonomous (non-`User`) turns.
//!
//! An injected turn (a promoted subagent result re-entering the parent —
//! Phase 3 — or an inter-agent `send_message` delivery — Phase 4) runs
//! with NO human attached to answer an `Ask` prompt. Left unguarded, an
//! `Ask` decision would either hang forever (no answerer) or, worse,
//! block on a channel that never resolves. This shim wraps any
//! `PermissionManager` and rewrites every `Ask` decision to `Deny` so an
//! autonomous turn can never park on a human prompt. `Allow` / `Deny`
//! pass through unchanged.
//!
//! Security (Phase 2 Findings 6/14): injected content is untrusted; it
//! must never be able to *escalate* by triggering an interactive prompt
//! that a confused human might approve out of context. Fail-closed Deny
//! removes that path entirely.

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

/// Wraps a parent [`PermissionManager`], converting any `Ask` decision to
/// a fail-closed `Deny`. Applied ONLY to turns whose origin is not
/// `User` (see `TurnOrigin` in the server crate). Every other API call
/// forwards to the inner manager unchanged.
pub struct FailClosedAskManager {
    inner: Arc<dyn PermissionManager>,
}

impl FailClosedAskManager {
    #[must_use]
    pub fn new(inner: Arc<dyn PermissionManager>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl PermissionManager for FailClosedAskManager {
    async fn check(
        &self,
        ctx: PermissionCtx,
        req: PermissionRequest,
    ) -> Result<Decision, PermissionError> {
        match self.inner.check(ctx, req).await? {
            // No human is attached to an autonomous injected turn — a
            // `Pending` (interactive ask) can neither be answered nor
            // safely auto-approved. Cancel the registered ask so the
            // inner manager doesn't leak a dangling rendezvous, then fail
            // closed to `Deny`.
            Decision::Pending { ask_id } => {
                let _ = self.inner.cancel_ask(ask_id).await;
                Ok(Decision::Deny {
                    feedback: Some(
                        "autonomous (injected) turn: interactive approval unavailable, denied"
                            .into(),
                    ),
                })
            }
            other => Ok(other),
        }
    }

    async fn reply(&self, ask_id: AskId, decision: Decision) -> Result<(), PermissionError> {
        self.inner.reply(ask_id, decision).await
    }

    async fn cancel_ask(&self, ask_id: AskId) -> Result<(), PermissionError> {
        self.inner.cancel_ask(ask_id).await
    }

    async fn record_always(
        &self,
        scope: AlwaysScope,
        rule: PermissionRule,
    ) -> Result<(), PermissionError> {
        self.inner.record_always(scope, rule).await
    }

    fn take_deferred(&self, ask_id: AskId) -> Option<Deferred<Decision>> {
        self.inner.take_deferred(ask_id)
    }

    fn peek_session_id(&self, ask_id: AskId) -> Option<SessionId> {
        self.inner.peek_session_id(ask_id)
    }

    async fn accept_ask(
        &self,
        ask_id: AskId,
        scope: AlwaysScope,
        action: PermissionAction,
    ) -> Result<(), PermissionError> {
        self.inner.accept_ask(ask_id, scope, action).await
    }
}

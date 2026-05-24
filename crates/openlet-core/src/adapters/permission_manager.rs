use async_trait::async_trait;

use crate::error::PermissionError;
use crate::types::permission::{
    AlwaysScope, AskId, Decision, PermissionCtx, PermissionRequest, PermissionRule,
};

/// Permission gate consulted before any sensitive tool call.
///
/// Phase 4 implements `ConfigPermissionMgr` with the layered ruleset
/// from amendment §E (defaults ++ agent ++ workspace ++ session, last-match-wins).
#[async_trait]
pub trait PermissionManager: Send + Sync + 'static {
    async fn check(
        &self,
        ctx: PermissionCtx,
        req: PermissionRequest,
    ) -> Result<Decision, PermissionError>;

    /// Reply to an outstanding ask (e.g. user clicked Allow in TUI).
    async fn reply(&self, ask_id: AskId, decision: Decision) -> Result<(), PermissionError>;

    /// Cancel a pending ask (used by §E timeout path).
    async fn cancel_ask(&self, ask_id: AskId) -> Result<(), PermissionError>;

    /// Persist an "always" decision at the requested scope (§A new method).
    async fn record_always(
        &self,
        scope: AlwaysScope,
        rule: PermissionRule,
    ) -> Result<(), PermissionError>;
}

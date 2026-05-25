use async_trait::async_trait;

use crate::error::PermissionError;
use crate::permission::Deferred;
use crate::types::permission::{
    AlwaysScope, AskId, Decision, PermissionCtx, PermissionRequest, PermissionRule,
};
use crate::types::session::SessionId;

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

    /// Surrender the receiver half of an outstanding ask. The runtime
    /// calls this after `check()` returns `Decision::Pending`, then
    /// `.await`s the deferred. Returns `None` if the ask was already
    /// taken or never existed. Sync because it's just a map mutation.
    fn take_deferred(&self, ask_id: AskId) -> Option<Deferred<Decision>>;

    /// Read-only peek at a pending ask's session id. The HTTP route uses
    /// this to publish `PermissionResolved` to the correct session
    /// before consuming the ask via `accept_ask` / `reply`. Returns
    /// `None` if the ask was already consumed.
    fn peek_session_id(&self, ask_id: AskId) -> Option<SessionId>;

    /// Atomic ask acceptance: consumes the pending ask, persists the
    /// rule scoped to `scope`, pushes it onto the in-memory ruleset, and
    /// resolves the deferred with `Decision::Allow`. All-or-nothing — if
    /// persistence fails, the ask is restored and the user sees an error.
    /// The HTTP route NEVER constructs a rule from client input; the
    /// rule pattern comes from the original `PermissionRequest`.
    async fn accept_ask(
        &self,
        ask_id: AskId,
        scope: AlwaysScope,
    ) -> Result<(), PermissionError>;
}

//! Config-driven `PermissionManager` impl.
//!
//! Phase 1 stub. Phase 4 implements the layered ruleset (§E:
//! defaults ++ agent ++ workspace ++ session, last-match-wins).

use async_trait::async_trait;
use openlet_core::adapters::permission_manager::PermissionManager;
use openlet_core::error::PermissionError;
use openlet_core::types::permission::{
    AlwaysScope, AskId, Decision, PermissionCtx, PermissionRequest, PermissionRule,
};

#[derive(Debug, Default)]
pub struct ConfigPermissionMgr;

impl ConfigPermissionMgr {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl PermissionManager for ConfigPermissionMgr {
    async fn check(
        &self,
        _ctx: PermissionCtx,
        _req: PermissionRequest,
    ) -> Result<Decision, PermissionError> {
        Err(PermissionError::Unimplemented)
    }

    async fn reply(
        &self,
        _ask_id: AskId,
        _decision: Decision,
    ) -> Result<(), PermissionError> {
        Err(PermissionError::Unimplemented)
    }

    async fn cancel_ask(&self, _ask_id: AskId) -> Result<(), PermissionError> {
        Err(PermissionError::Unimplemented)
    }

    async fn record_always(
        &self,
        _scope: AlwaysScope,
        _rule: PermissionRule,
    ) -> Result<(), PermissionError> {
        Err(PermissionError::Unimplemented)
    }
}

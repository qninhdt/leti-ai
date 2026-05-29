//! Permission gate mocks for tests.
//!
//! Three behaviours:
//! - [`AllowAll`] — every check returns `Decision::Allow` immediately.
//! - [`DenyAll`] — every check returns `Decision::Deny { feedback }`.
//! - [`ScriptedPermission`] — pop the next decision from a `VecDeque`;
//!   panics if the queue empties (test author is expected to set up
//!   exactly the calls they expect).

use std::collections::VecDeque;
use std::sync::Mutex;

use async_trait::async_trait;
use openlet_core::adapters::permission_manager::PermissionManager;
use openlet_core::error::PermissionError;
use openlet_core::permission::Deferred;
use openlet_core::types::permission::{
    AlwaysScope, AskId, Decision, PermissionAction, PermissionCtx, PermissionRequest,
    PermissionRule,
};
use openlet_core::types::session::SessionId;

pub struct AllowAll;

#[async_trait]
impl PermissionManager for AllowAll {
    async fn check(
        &self,
        _ctx: PermissionCtx,
        _req: PermissionRequest,
    ) -> Result<Decision, PermissionError> {
        Ok(Decision::Allow)
    }
    async fn reply(&self, _ask_id: AskId, _decision: Decision) -> Result<(), PermissionError> {
        Ok(())
    }
    async fn cancel_ask(&self, _ask_id: AskId) -> Result<(), PermissionError> {
        Ok(())
    }
    async fn record_always(
        &self,
        _scope: AlwaysScope,
        _rule: PermissionRule,
    ) -> Result<(), PermissionError> {
        Ok(())
    }
    fn take_deferred(&self, _ask_id: AskId) -> Option<Deferred<Decision>> {
        None
    }
    fn peek_session_id(&self, _ask_id: AskId) -> Option<SessionId> {
        None
    }
    async fn accept_ask(
        &self,
        _ask_id: AskId,
        _scope: AlwaysScope,
        _action: PermissionAction,
    ) -> Result<(), PermissionError> {
        Ok(())
    }
}

pub struct DenyAll;

#[async_trait]
impl PermissionManager for DenyAll {
    async fn check(
        &self,
        _ctx: PermissionCtx,
        _req: PermissionRequest,
    ) -> Result<Decision, PermissionError> {
        Ok(Decision::Deny {
            feedback: Some("denied by DenyAll mock".into()),
        })
    }
    async fn reply(&self, _ask_id: AskId, _decision: Decision) -> Result<(), PermissionError> {
        Ok(())
    }
    async fn cancel_ask(&self, _ask_id: AskId) -> Result<(), PermissionError> {
        Ok(())
    }
    async fn record_always(
        &self,
        _scope: AlwaysScope,
        _rule: PermissionRule,
    ) -> Result<(), PermissionError> {
        Ok(())
    }
    fn take_deferred(&self, _ask_id: AskId) -> Option<Deferred<Decision>> {
        None
    }
    fn peek_session_id(&self, _ask_id: AskId) -> Option<SessionId> {
        None
    }
    async fn accept_ask(
        &self,
        _ask_id: AskId,
        _scope: AlwaysScope,
        _action: PermissionAction,
    ) -> Result<(), PermissionError> {
        Ok(())
    }
}

pub struct ScriptedPermission {
    queue: Mutex<VecDeque<Decision>>,
}

impl ScriptedPermission {
    #[must_use]
    pub fn new(decisions: impl IntoIterator<Item = Decision>) -> Self {
        Self {
            queue: Mutex::new(decisions.into_iter().collect()),
        }
    }

    pub fn push(&self, d: Decision) -> &Self {
        self.queue.lock().unwrap().push_back(d);
        self
    }

    pub fn remaining(&self) -> usize {
        self.queue.lock().unwrap().len()
    }
}

#[async_trait]
impl PermissionManager for ScriptedPermission {
    async fn check(
        &self,
        _ctx: PermissionCtx,
        _req: PermissionRequest,
    ) -> Result<Decision, PermissionError> {
        let next = self
            .queue
            .lock()
            .unwrap()
            .pop_front()
            .expect("ScriptedPermission queue empty");
        Ok(next)
    }
    async fn reply(&self, _ask_id: AskId, _decision: Decision) -> Result<(), PermissionError> {
        Ok(())
    }
    async fn cancel_ask(&self, _ask_id: AskId) -> Result<(), PermissionError> {
        Ok(())
    }
    async fn record_always(
        &self,
        _scope: AlwaysScope,
        _rule: PermissionRule,
    ) -> Result<(), PermissionError> {
        Ok(())
    }
    fn take_deferred(&self, _ask_id: AskId) -> Option<Deferred<Decision>> {
        None
    }
    fn peek_session_id(&self, _ask_id: AskId) -> Option<SessionId> {
        None
    }
    async fn accept_ask(
        &self,
        _ask_id: AskId,
        _scope: AlwaysScope,
        _action: PermissionAction,
    ) -> Result<(), PermissionError> {
        Ok(())
    }
}

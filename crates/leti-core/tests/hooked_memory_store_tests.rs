//! End-to-end tests for `HookedMemoryStore` — covers OnMessage and
//! OnSessionStatus dispatch sites through the decorator.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::Utc;
use leti_core::adapters::hooked_memory_store::HookedMemoryStore;
use leti_core::adapters::memory_store::MemoryStore;
use leti_core::dispatch::{HookChains, HookEntry};
use leti_core::error::MemoryError;
use leti_core::hooks::{
    HookKind, HookResult, Priority,
    io::{OnMessageCtx, OnSessionStatusCtx},
};
use leti_core::types::agent::AgentId;
use leti_core::types::message::{Message, MessageId, Role};
use leti_core::types::part::{Part, PartId};
use leti_core::types::permission::PermissionMode;
use leti_core::types::session::{SessionFilter, SessionId, SessionMeta, SessionStatus};
use std::path::PathBuf;

#[derive(Default)]
struct StubInner {
    messages: Mutex<Vec<(SessionId, Message)>>,
    statuses: Mutex<Vec<(SessionId, SessionStatus, String)>>,
}

#[async_trait]
impl MemoryStore for StubInner {
    async fn create_session(
        &self,
        _: AgentId,
        _: Option<SessionId>,
    ) -> Result<SessionId, MemoryError> {
        Ok(SessionId::new())
    }
    async fn get_session(&self, _: SessionId) -> Result<Option<SessionMeta>, MemoryError> {
        Ok(None)
    }
    async fn list_sessions(&self, _: SessionFilter) -> Result<Vec<SessionMeta>, MemoryError> {
        Ok(vec![])
    }
    async fn update_status(
        &self,
        s: SessionId,
        status: SessionStatus,
        reason: &str,
    ) -> Result<(), MemoryError> {
        self.statuses
            .lock()
            .unwrap()
            .push((s, status, reason.to_string()));
        Ok(())
    }
    async fn update_permission_mode(
        &self,
        _: SessionId,
        _: PermissionMode,
    ) -> Result<(), MemoryError> {
        Ok(())
    }
    async fn switch_agent(&self, _: SessionId, _: &str) -> Result<(), MemoryError> {
        Ok(())
    }
    async fn update_session_extensions(
        &self,
        _: SessionId,
        _: serde_json::Value,
    ) -> Result<(), MemoryError> {
        Ok(())
    }
    async fn delete_session(&self, _: SessionId) -> Result<(), MemoryError> {
        Ok(())
    }
    async fn append_message(&self, s: SessionId, msg: Message) -> Result<MessageId, MemoryError> {
        let id = msg.id;
        self.messages.lock().unwrap().push((s, msg));
        Ok(id)
    }
    async fn append_part(&self, _: MessageId, _: Part) -> Result<PartId, MemoryError> {
        Ok(PartId::new())
    }
    async fn upsert_part(&self, _: MessageId, _: PartId, _: Part) -> Result<(), MemoryError> {
        Ok(())
    }
    async fn list_messages(&self, _: SessionId) -> Result<Vec<Message>, MemoryError> {
        Ok(vec![])
    }
    async fn list_parts(&self, _: SessionId, _: MessageId) -> Result<Vec<Part>, MemoryError> {
        Ok(vec![])
    }
    async fn record_read(&self, _: SessionId, _: PathBuf) -> Result<(), MemoryError> {
        Ok(())
    }
}

fn fresh_msg(sid: SessionId) -> Message {
    Message {
        id: MessageId::new(),
        session_id: sid,
        role: Role::User,
        created_at: Utc::now(),
    }
}

#[tokio::test]
async fn append_message_dispatches_on_message_after_persist() {
    let inner = Arc::new(StubInner::default());
    let observed = Arc::new(Mutex::new(Vec::<MessageId>::new()));
    let observed_clone = observed.clone();

    let mut chains = HookChains::new();
    chains.on_message.push(HookEntry::<OnMessageCtx> {
        manifest_id: "audit".into(),
        priority: Priority(50),
        registration_index: 0,
        kind: HookKind::OnMessage,
        func: Arc::new(move |c: OnMessageCtx| {
            let observed = observed_clone.clone();
            Box::pin(async move {
                if let Some(m) = c.message.as_ref() {
                    observed.lock().unwrap().push(m.id);
                }
                HookResult::Continue(c)
            })
        }),
    });

    let store: Arc<dyn MemoryStore> =
        Arc::new(HookedMemoryStore::new(inner.clone(), Arc::new(chains)));
    let sid = SessionId::new();
    let msg = fresh_msg(sid);
    let returned_id = store.append_message(sid, msg).await.unwrap();

    assert_eq!(inner.messages.lock().unwrap().len(), 1);
    assert_eq!(observed.lock().unwrap().as_slice(), &[returned_id]);
}

#[tokio::test]
async fn empty_chain_skips_on_message_dispatch() {
    let inner = Arc::new(StubInner::default());
    let store: Arc<dyn MemoryStore> = Arc::new(HookedMemoryStore::new(
        inner.clone(),
        Arc::new(HookChains::new()),
    ));
    let sid = SessionId::new();
    let _ = store.append_message(sid, fresh_msg(sid)).await.unwrap();
    assert_eq!(inner.messages.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn update_status_dispatches_on_session_status() {
    let inner = Arc::new(StubInner::default());
    let observed = Arc::new(Mutex::new(Vec::<SessionStatus>::new()));
    let observed_clone = observed.clone();

    let mut chains = HookChains::new();
    chains
        .on_session_status
        .push(HookEntry::<OnSessionStatusCtx> {
            manifest_id: "lifecycle".into(),
            priority: Priority(50),
            registration_index: 0,
            kind: HookKind::OnSessionStatus,
            func: Arc::new(move |c: OnSessionStatusCtx| {
                let observed = observed_clone.clone();
                Box::pin(async move {
                    if let Some(s) = c.status {
                        observed.lock().unwrap().push(s);
                    }
                    HookResult::Continue(c)
                })
            }),
        });

    let store: Arc<dyn MemoryStore> =
        Arc::new(HookedMemoryStore::new(inner.clone(), Arc::new(chains)));
    let sid = SessionId::new();
    store
        .update_status(sid, SessionStatus::Running, "boot")
        .await
        .unwrap();
    store
        .update_status(sid, SessionStatus::Cancelled, "done")
        .await
        .unwrap();

    assert_eq!(
        observed.lock().unwrap().as_slice(),
        &[SessionStatus::Running, SessionStatus::Cancelled],
    );
    assert_eq!(inner.statuses.lock().unwrap().len(), 2);
}

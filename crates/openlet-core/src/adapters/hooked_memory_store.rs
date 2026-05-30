//! [`HookedMemoryStore`] — wraps an inner [`MemoryStore`] and runs the
//! `on_message` hook chain after `append_message` succeeds.
//!
//! Wired as a decorator at boot so call sites in the runtime + handlers
//! never need to dispatch directly. Same shape as [`HookedEventSink`].
//!
//! `on_message` runs AFTER the message is durable so audit/derived-store
//! plugins always observe the same state the runtime sees. The chain's
//! `Replace`/`Stop`/`Deny` outcomes are observed but cannot mutate the
//! persisted record — the hook is read-only with respect to storage.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;

use crate::adapters::event_sink::EventSink;
use crate::adapters::memory_store::MemoryStore;
use crate::dispatch::{HookChains, dispatch, publish_fault_if_any};
use crate::error::MemoryError;
use crate::hooks::io::{OnMessageCtx, OnSessionStatusCtx};
use crate::types::agent::AgentId;
use crate::types::message::{Message, MessageId};
use crate::types::part::{Part, PartId};
use crate::types::permission::PermissionMode;
use crate::types::session::{SessionFilter, SessionId, SessionMeta, SessionStatus};

pub struct HookedMemoryStore {
    inner: Arc<dyn MemoryStore>,
    hook_chains: Arc<HookChains>,
    events: Option<Arc<dyn EventSink>>,
}

impl HookedMemoryStore {
    #[must_use]
    pub fn new(inner: Arc<dyn MemoryStore>, hook_chains: Arc<HookChains>) -> Self {
        Self {
            inner,
            hook_chains,
            events: None,
        }
    }

    /// Same as [`Self::new`] but wires a sink so synthetic denies
    /// (panic / timeout) emit `AgentEvent::PluginError` for cloud-grep
    /// telemetry. Existing callers without a sink fall back to silent
    /// drop, matching the pre-3c behavior.
    #[must_use]
    pub fn with_events(
        inner: Arc<dyn MemoryStore>,
        hook_chains: Arc<HookChains>,
        events: Arc<dyn EventSink>,
    ) -> Self {
        Self {
            inner,
            hook_chains,
            events: Some(events),
        }
    }
}

#[async_trait]
impl MemoryStore for HookedMemoryStore {
    async fn create_session(
        &self,
        agent_id: AgentId,
        parent: Option<SessionId>,
    ) -> Result<SessionId, MemoryError> {
        self.inner.create_session(agent_id, parent).await
    }

    async fn create_session_with_meta(
        &self,
        meta: SessionMeta,
    ) -> Result<SessionId, MemoryError> {
        self.inner.create_session_with_meta(meta).await
    }

    async fn get_session(&self, session: SessionId) -> Result<Option<SessionMeta>, MemoryError> {
        self.inner.get_session(session).await
    }

    async fn list_sessions(&self, filter: SessionFilter) -> Result<Vec<SessionMeta>, MemoryError> {
        self.inner.list_sessions(filter).await
    }

    async fn update_status(
        &self,
        session: SessionId,
        status: SessionStatus,
        reason: &str,
    ) -> Result<(), MemoryError> {
        self.inner.update_status(session, status, reason).await?;
        if !self.hook_chains.on_session_status.is_empty() {
            let ctx = OnSessionStatusCtx {
                session_id: Some(session),
                status: Some(status),
            };
            let outcome = dispatch(&self.hook_chains.on_session_status, ctx).await;
            if let Some(events) = self.events.as_ref() {
                publish_fault_if_any(events, Some(session), &outcome).await;
            }
        }
        Ok(())
    }

    async fn update_permission_mode(
        &self,
        session: SessionId,
        mode: PermissionMode,
    ) -> Result<(), MemoryError> {
        self.inner.update_permission_mode(session, mode).await
    }

    async fn switch_agent(&self, session: SessionId, agent_slug: &str) -> Result<(), MemoryError> {
        self.inner.switch_agent(session, agent_slug).await
    }

    async fn update_session_extensions(
        &self,
        session: SessionId,
        extensions: serde_json::Value,
    ) -> Result<(), MemoryError> {
        self.inner
            .update_session_extensions(session, extensions)
            .await
    }

    async fn delete_session(&self, session: SessionId) -> Result<(), MemoryError> {
        self.inner.delete_session(session).await
    }

    async fn append_message(
        &self,
        session: SessionId,
        msg: Message,
    ) -> Result<MessageId, MemoryError> {
        // Skip dispatch entirely when no plugin registered the chain —
        // avoids the per-message clone on the hot path.
        if self.hook_chains.on_message.is_empty() {
            return self.inner.append_message(session, msg).await;
        }
        let id = self.inner.append_message(session, msg.clone()).await?;
        let ctx = OnMessageCtx {
            session_id: Some(session),
            message: Some(Message { id, ..msg }),
        };
        // Observation: outcome is intentionally discarded. Storage is
        // already durable; plugins cannot un-persist the message.
        let _ = dispatch(&self.hook_chains.on_message, ctx).await;
        Ok(id)
    }

    async fn append_part(&self, msg: MessageId, part: Part) -> Result<PartId, MemoryError> {
        self.inner.append_part(msg, part).await
    }

    async fn upsert_part(
        &self,
        msg: MessageId,
        part_id: PartId,
        part: Part,
    ) -> Result<(), MemoryError> {
        self.inner.upsert_part(msg, part_id, part).await
    }

    async fn list_messages(&self, session: SessionId) -> Result<Vec<Message>, MemoryError> {
        self.inner.list_messages(session).await
    }

    async fn list_parts(
        &self,
        session: SessionId,
        msg: MessageId,
    ) -> Result<Vec<Part>, MemoryError> {
        self.inner.list_parts(session, msg).await
    }

    async fn record_read(&self, session: SessionId, path: PathBuf) -> Result<(), MemoryError> {
        self.inner.record_read(session, path).await
    }
}

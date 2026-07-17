//! Shared `ToolCtx` builder for executor integration tests.
//!
//! The executor traits (`ShellExecutor`, and later `PythonExecutor`) only
//! touch `ctx.fs` and `ctx.cancel`, but `ToolCtx` is a wide struct. Rather
//! than duplicate ~150 lines of no-op adapter impls per test file, both
//! the `localshell` and `emushell` suites build their context here.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use leti_adapters::localfs::LocalFilesystem;
use leti_core::adapters::artifact_store::{ArtifactRef, ArtifactStore};
use leti_core::adapters::event_sink::{EventSink, Persistence};
use leti_core::adapters::permission_manager::PermissionManager;
use leti_core::adapters::tool_executor::ToolCtx;
use leti_core::error::{ArtifactError, EventError, PermissionError};
use leti_core::tools::ReadHistory;
use leti_core::types::agent::AgentId;
use leti_core::types::event::{AgentEvent, EventFilter};
use leti_core::types::message::MessageId;
use leti_core::types::permission::{
    AlwaysScope, AskId, Decision, PermissionAction, PermissionCtx, PermissionMode,
    PermissionRequest, PermissionRule,
};
use leti_core::types::session::SessionId;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub struct AllowAll;

#[async_trait]
impl PermissionManager for AllowAll {
    async fn check(
        &self,
        _: PermissionCtx,
        _: PermissionRequest,
    ) -> Result<Decision, PermissionError> {
        Ok(Decision::Allow)
    }
    async fn reply(&self, _: AskId, _: Decision) -> Result<(), PermissionError> {
        Ok(())
    }
    async fn cancel_ask(&self, _: AskId) -> Result<(), PermissionError> {
        Ok(())
    }
    async fn record_always(
        &self,
        _: AlwaysScope,
        _: PermissionRule,
    ) -> Result<(), PermissionError> {
        Ok(())
    }
    fn take_deferred(&self, _: AskId) -> Option<leti_core::permission::Deferred<Decision>> {
        None
    }
    fn peek_session_id(&self, _: AskId) -> Option<SessionId> {
        None
    }
    async fn accept_ask(
        &self,
        _: AskId,
        _: AlwaysScope,
        _: PermissionAction,
    ) -> Result<(), PermissionError> {
        Ok(())
    }
}

#[derive(Default)]
pub struct NoopBus;

#[async_trait]
impl EventSink for NoopBus {
    async fn publish(&self, _: AgentEvent, _: Persistence) -> Result<(), EventError> {
        Ok(())
    }
    fn subscribe(
        &self,
        _: EventFilter,
    ) -> broadcast::Receiver<leti_core::adapters::event_sink::DeliveredEvent> {
        let (_, rx) = broadcast::channel(1);
        rx
    }
}

#[derive(Default)]
pub struct DiscardArtifacts;

#[async_trait]
impl ArtifactStore for DiscardArtifacts {
    async fn put(
        &self,
        session: SessionId,
        key: &str,
        _: Bytes,
    ) -> Result<ArtifactRef, ArtifactError> {
        Ok(ArtifactRef {
            session_id: session,
            key: key.to_string(),
            size: 0,
            mime: None,
        })
    }
    async fn get(&self, _: &ArtifactRef) -> Result<Bytes, ArtifactError> {
        Err(ArtifactError::NotFound("test".into()))
    }
    async fn list(&self, _: SessionId) -> Result<Vec<ArtifactRef>, ArtifactError> {
        Ok(vec![])
    }
}

fn noop_memory() -> Arc<dyn leti_core::adapters::memory_store::MemoryStore> {
    use leti_core::adapters::memory_store::MemoryStore;
    use leti_core::error::MemoryError;

    struct NoopMemory;

    #[async_trait]
    impl MemoryStore for NoopMemory {
        async fn create_session(
            &self,
            _: AgentId,
            _: Option<SessionId>,
        ) -> Result<SessionId, MemoryError> {
            Err(MemoryError::Unimplemented)
        }
        async fn get_session(
            &self,
            _: SessionId,
        ) -> Result<Option<leti_core::types::session::SessionMeta>, MemoryError> {
            Ok(None)
        }
        async fn list_sessions(
            &self,
            _: leti_core::types::session::SessionFilter,
        ) -> Result<Vec<leti_core::types::session::SessionMeta>, MemoryError> {
            Ok(vec![])
        }
        async fn update_status(
            &self,
            _: SessionId,
            _: leti_core::types::session::SessionStatus,
            _: &str,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn switch_agent(&self, _: SessionId, _: &str) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn update_permission_mode(
            &self,
            _: SessionId,
            _: PermissionMode,
        ) -> Result<(), MemoryError> {
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
        async fn append_message(
            &self,
            _: SessionId,
            msg: leti_core::types::message::Message,
        ) -> Result<MessageId, MemoryError> {
            Ok(msg.id)
        }
        async fn append_part(
            &self,
            _: MessageId,
            _: leti_core::types::part::Part,
        ) -> Result<leti_core::types::part::PartId, MemoryError> {
            Ok(leti_core::types::part::PartId::new())
        }
        async fn upsert_part(
            &self,
            _: MessageId,
            _: leti_core::types::part::PartId,
            _: leti_core::types::part::Part,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn list_messages(
            &self,
            _: SessionId,
        ) -> Result<Vec<leti_core::types::message::Message>, MemoryError> {
            Ok(vec![])
        }
        async fn list_parts(
            &self,
            _: SessionId,
            _: MessageId,
        ) -> Result<Vec<leti_core::types::part::Part>, MemoryError> {
            Ok(vec![])
        }
        async fn record_read(
            &self,
            _: SessionId,
            _: std::path::PathBuf,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
    }

    Arc::new(NoopMemory)
}

/// Build a `ToolCtx` rooted at `workspace` with all-permissive adapters.
/// Only `fs` and `cancel` carry real behavior; everything else is a no-op.
pub fn tool_ctx(workspace: &Path, cancel: CancellationToken) -> ToolCtx {
    tool_ctx_with_fs(
        Arc::new(LocalFilesystem::new(workspace.to_path_buf())),
        cancel,
    )
}

/// Same as [`tool_ctx`] but with a caller-supplied `Filesystem` impl. The
/// parity suite uses this to run the SAME interpreter against `LocalFilesystem`
/// and an in-memory FS and assert byte-identical output — proving the executor
/// is filesystem-impl-agnostic (the local=cloud thesis of the plan).
pub fn tool_ctx_with_fs(
    fs: Arc<dyn leti_core::adapters::Filesystem>,
    cancel: CancellationToken,
) -> ToolCtx {
    ToolCtx {
        ext: Default::default(),
        session_id: SessionId::new(),
        agent_id: AgentId::new(),
        message_id: MessageId::new(),
        call_id: "emushell-test".into(),
        fs,
        mode: PermissionMode::Danger,
        permission: Arc::new(AllowAll),
        events: Arc::new(NoopBus),
        artifacts: Arc::new(DiscardArtifacts),
        read_history: ReadHistory::new(),
        cancel,
        questions: Arc::new(leti_core::runtime::QuestionRegistry::new()),
        memory: noop_memory(),
        task_registry: Arc::new(leti_core::runtime::subagent::TaskRegistry::new(32)),
        agent_registry: Arc::new(leti_core::agent::AgentRegistry::new()),
    }
}

//! Tests for the `ask_user` builtin tool — capability gate, per-session
//! cap, timeout, registry single-use semantics.

#![allow(clippy::needless_pass_by_value)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use chrono::Utc;
use openlet_adapters::localfs::LocalFilesystem;
use openlet_core::adapters::artifact_store::{ArtifactRef, ArtifactStore};
use openlet_core::adapters::event_sink::{DeliveredEvent, EventSink, Persistence};
use openlet_core::adapters::memory_store::MemoryStore;
use openlet_core::adapters::permission_manager::PermissionManager;
use openlet_core::adapters::tool_executor::ToolCtx;
use openlet_core::error::{ArtifactError, EventError, MemoryError, PermissionError, ToolError};
use openlet_core::runtime::QuestionRegistry;
use openlet_core::tools::ReadHistory;
use openlet_core::tools::Tool;
use openlet_core::tools::builtins::ask_user::{AskOptionInput, AskUserInput, AskUserTool};
use openlet_core::types::agent::AgentId;
use openlet_core::types::event::{AgentEvent, EventFilter};
use openlet_core::types::message::{Message, MessageId};
use openlet_core::types::part::{Part, PartId};
use openlet_core::types::permission::{
    AlwaysScope, AskId, Decision, PermissionAction, PermissionMode, PermissionRequest,
};
use openlet_core::types::session::{
    SessionCapabilities, SessionFilter, SessionId, SessionMeta, SessionStatus,
};
use tempfile::TempDir;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn returns_unavailable_when_capability_off() {
    let memory = Arc::new(StubMemory::with_capability(false));
    let ctx = test_ctx(memory.clone(), CancellationToken::new());
    let tool = AskUserTool::new();

    let err = Tool::run(&tool, ctx, valid_input())
        .await
        .expect_err("should error synchronously when capability=false");
    match err {
        ToolError::InvalidInput(code) => {
            assert_eq!(code, "user_questions_unavailable_in_session");
        }
        other => panic!("expected InvalidInput, got {other:?}"),
    }
}

#[tokio::test]
async fn returns_already_pending_on_concurrent_call() {
    let memory = Arc::new(StubMemory::with_capability(true));
    let questions = Arc::new(QuestionRegistry::new());
    let session_id = SessionId::new();

    // First call holds the slot — drive it through a long timeout so it
    // hangs on the receiver. Spawn it; second call should immediately
    // surface `question_already_pending`.
    let ctx_a = test_ctx_with(
        memory.clone(),
        questions.clone(),
        session_id,
        CancellationToken::new(),
        Duration::from_secs(60),
    );
    let tool_a = AskUserTool::with_timeout(Duration::from_secs(60));
    let task = tokio::spawn(async move { Tool::run(&tool_a, ctx_a, valid_input()).await });

    // Yield until the slot is observably held.
    for _ in 0..200 {
        if questions.pending_len() > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(questions.pending_len() > 0, "first call should hold slot");

    let ctx_b = test_ctx_with(
        memory.clone(),
        questions.clone(),
        session_id,
        CancellationToken::new(),
        Duration::from_millis(50),
    );
    let tool_b = AskUserTool::with_timeout(Duration::from_millis(50));
    let err = Tool::run(&tool_b, ctx_b, valid_input())
        .await
        .expect_err("second call must reject");
    match err {
        ToolError::InvalidInput(code) => assert_eq!(code, "question_already_pending"),
        other => panic!("expected InvalidInput, got {other:?}"),
    }

    // Cancel the first call so the test exits.
    task.abort();
    let _ = task.await;
}

#[tokio::test]
async fn returns_timeout_when_no_reply_arrives() {
    let memory = Arc::new(StubMemory::with_capability(true));
    let questions = Arc::new(QuestionRegistry::new());
    let session_id = SessionId::new();
    let ctx = test_ctx_with(
        memory,
        questions.clone(),
        session_id,
        CancellationToken::new(),
        Duration::from_millis(10),
    );
    let tool = AskUserTool::with_timeout(Duration::from_millis(10));

    let err = Tool::run(&tool, ctx, valid_input())
        .await
        .expect_err("must time out");
    assert!(matches!(err, ToolError::Timeout));
    // Slot must be released after the timeout so subsequent calls work.
    assert_eq!(questions.pending_len(), 0);
}

/// When NO answer has arrived, a pre-cancelled token (operator kill /
/// consent revocation = `CancelReason::SessionEnding`) MUST win: the tool
/// returns the cancelled error, never serving an answer for a session the
/// operator aborted. This is the consent-preserving half of the behavior;
/// the already-delivered-answer half is unit-tested in `ask_user_runner`.
#[tokio::test]
async fn cancel_wins_when_no_answer_pending() {
    let memory = Arc::new(StubMemory::with_capability(true));
    let questions = Arc::new(QuestionRegistry::new());
    let session_id = SessionId::new();
    // Token already cancelled before the tool runs — no answer will ever
    // be delivered.
    let cancel = CancellationToken::new();
    cancel.cancel();
    let ctx = test_ctx_with(
        memory,
        questions.clone(),
        session_id,
        cancel,
        Duration::from_secs(60),
    );
    let tool = AskUserTool::with_timeout(Duration::from_secs(60));

    let err = Tool::run(&tool, ctx, valid_input())
        .await
        .expect_err("cancelled session must not serve an answer");
    match err {
        ToolError::InvalidInput(code) => assert_eq!(code, "question_cancelled"),
        other => panic!("expected question_cancelled, got {other:?}"),
    }
    // Slot released on the cancel path too.
    assert_eq!(questions.pending_len(), 0);
}

fn valid_input() -> AskUserInput {
    AskUserInput {
        header: "Choose".to_string(),
        question: "Pick one".to_string(),
        options: vec![
            AskOptionInput {
                label: "Yes".into(),
                description: None,
            },
            AskOptionInput {
                label: "No".into(),
                description: None,
            },
        ],
        multi_select: false,
    }
}

fn test_ctx(memory: Arc<dyn MemoryStore>, cancel: CancellationToken) -> ToolCtx {
    let questions = Arc::new(QuestionRegistry::new());
    test_ctx_with(
        memory,
        questions,
        SessionId::new(),
        cancel,
        Duration::from_secs(60),
    )
}

fn test_ctx_with(
    memory: Arc<dyn MemoryStore>,
    questions: Arc<QuestionRegistry>,
    session_id: SessionId,
    cancel: CancellationToken,
    _timeout: Duration,
) -> ToolCtx {
    let workspace = TempDir::new().expect("tempdir").path().to_path_buf();
    ToolCtx {
        session_id,
        agent_id: AgentId::new(),
        message_id: MessageId::new(),
        call_id: "call-ask-user".into(),
        fs: Arc::new(LocalFilesystem::new(workspace)),
        mode: PermissionMode::Danger,
        permission: Arc::new(AllowAll),
        events: Arc::new(NoopBus::default()),
        artifacts: Arc::new(DiscardArtifacts),
        read_history: ReadHistory::new(),
        cancel,
        questions,
        memory,
        task_registry: Arc::new(openlet_core::runtime::subagent::TaskRegistry::new(32)),
        agent_registry: Arc::new(openlet_core::agent::AgentRegistry::new()),
    }
}

struct AllowAll;

#[async_trait]
impl PermissionManager for AllowAll {
    async fn check(
        &self,
        _: openlet_core::types::permission::PermissionCtx,
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
        _: openlet_core::types::permission::PermissionRule,
    ) -> Result<(), PermissionError> {
        Ok(())
    }
    fn take_deferred(&self, _: AskId) -> Option<openlet_core::permission::Deferred<Decision>> {
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
struct NoopBus {
    tx: Option<broadcast::Sender<DeliveredEvent>>,
}

#[async_trait]
impl EventSink for NoopBus {
    async fn publish(&self, _: AgentEvent, _: Persistence) -> Result<(), EventError> {
        Ok(())
    }
    fn subscribe(&self, _: EventFilter) -> broadcast::Receiver<DeliveredEvent> {
        if let Some(tx) = &self.tx {
            tx.subscribe()
        } else {
            broadcast::channel(1).1
        }
    }
}

struct DiscardArtifacts;

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

struct StubMemory {
    capability: bool,
}

impl StubMemory {
    fn with_capability(capability: bool) -> Self {
        Self { capability }
    }
}

#[async_trait]
impl MemoryStore for StubMemory {
    async fn create_session(
        &self,
        _: AgentId,
        _: Option<SessionId>,
    ) -> Result<SessionId, MemoryError> {
        Err(MemoryError::Unimplemented)
    }
    async fn get_session(&self, id: SessionId) -> Result<Option<SessionMeta>, MemoryError> {
        let caps = SessionCapabilities {
            user_questions: self.capability,
        };
        Ok(Some(SessionMeta {
            id,
            agent_id: AgentId::new(),
            status: SessionStatus::Running,
            permission_mode: PermissionMode::Danger,
            parent_session_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deleted_at: None,
            version: "0.1.0".into(),
            extensions: serde_json::Value::Null,
            capabilities: caps,
            current_agent_slug: None,
            previous_agent_slug: None,
            depth: 0,
        }))
    }
    async fn list_sessions(&self, _: SessionFilter) -> Result<Vec<SessionMeta>, MemoryError> {
        Ok(vec![])
    }
    async fn update_status(
        &self,
        _: SessionId,
        _: SessionStatus,
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
    async fn append_message(&self, _: SessionId, msg: Message) -> Result<MessageId, MemoryError> {
        Ok(msg.id)
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

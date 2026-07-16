//! Read-before-write enforcement — write/edit refuse without prior read,
//! succeed after read, and bypass in Danger mode.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use openlet_adapters::localfs::LocalFilesystem;
use openlet_core::adapters::artifact_store::{ArtifactRef, ArtifactStore};
use openlet_core::adapters::event_sink::{EventSink, Persistence};
use openlet_core::adapters::permission_manager::PermissionManager;
use openlet_core::adapters::tool_executor::ToolCtx;
use openlet_core::error::{ArtifactError, EventError, PermissionError, ToolError};
use openlet_core::tools::builtins::edit::EditInput;
use openlet_core::tools::builtins::read::ReadInput;
use openlet_core::tools::builtins::write::WriteInput;
use openlet_core::tools::builtins::{EditTool, ReadTool, WriteTool};
use openlet_core::tools::{ReadHistory, Tool};
use openlet_core::types::agent::AgentId;
use openlet_core::types::event::{AgentEvent, EventFilter};
use openlet_core::types::message::MessageId;
use openlet_core::types::permission::{
    AlwaysScope, AskId, Decision, PermissionAction, PermissionCtx, PermissionMode,
    PermissionRequest, PermissionRule,
};
use openlet_core::types::session::SessionId;
use tempfile::TempDir;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct AllowAll;

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
    fn take_deferred(&self, _: AskId) -> Option<openlet_core::permission::Deferred<Decision>> {
        None
    }
    fn peek_session_id(&self, _: AskId) -> Option<openlet_core::types::session::SessionId> {
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
struct NoopBus;

#[async_trait]
impl EventSink for NoopBus {
    async fn publish(&self, _: AgentEvent, _: Persistence) -> Result<(), EventError> {
        Ok(())
    }
    fn subscribe(
        &self,
        _: EventFilter,
    ) -> broadcast::Receiver<openlet_core::adapters::event_sink::DeliveredEvent> {
        let (_, rx) = broadcast::channel(1);
        rx
    }
}

#[derive(Default)]
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

fn ctx(workspace: &Path, mode: PermissionMode, history: ReadHistory) -> ToolCtx {
    ToolCtx {
        session_id: SessionId::new(),
        agent_id: AgentId::new(),
        message_id: MessageId::new(),
        call_id: "call-1".into(),
        fs: Arc::new(LocalFilesystem::new(workspace.to_path_buf())),
        mode,
        permission: Arc::new(AllowAll),
        events: Arc::new(NoopBus),
        artifacts: Arc::new(DiscardArtifacts),
        read_history: history,
        cancel: CancellationToken::new(),
        questions: Arc::new(openlet_core::runtime::QuestionRegistry::new()),
        memory: noop_memory(),
        task_registry: Arc::new(openlet_core::runtime::subagent::TaskRegistry::new(32)),
        agent_registry: Arc::new(openlet_core::agent::AgentRegistry::new()),
    }
}

fn noop_memory() -> Arc<dyn openlet_core::adapters::memory_store::MemoryStore> {
    use openlet_core::adapters::memory_store::MemoryStore;
    use openlet_core::error::MemoryError;

    struct NoopMemory;

    #[async_trait::async_trait]
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
        ) -> Result<Option<openlet_core::types::session::SessionMeta>, MemoryError> {
            Ok(None)
        }
        async fn list_sessions(
            &self,
            _: openlet_core::types::session::SessionFilter,
        ) -> Result<Vec<openlet_core::types::session::SessionMeta>, MemoryError> {
            Ok(vec![])
        }
        async fn update_status(
            &self,
            _: SessionId,
            _: openlet_core::types::session::SessionStatus,
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
            msg: openlet_core::types::message::Message,
        ) -> Result<MessageId, MemoryError> {
            Ok(msg.id)
        }
        async fn append_part(
            &self,
            _: MessageId,
            _: openlet_core::types::part::Part,
        ) -> Result<openlet_core::types::part::PartId, MemoryError> {
            Ok(openlet_core::types::part::PartId::new())
        }
        async fn upsert_part(
            &self,
            _: MessageId,
            _: openlet_core::types::part::PartId,
            _: openlet_core::types::part::Part,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn list_messages(
            &self,
            _: SessionId,
        ) -> Result<Vec<openlet_core::types::message::Message>, MemoryError> {
            Ok(vec![])
        }
        async fn list_parts(
            &self,
            _: SessionId,
            _: MessageId,
        ) -> Result<Vec<openlet_core::types::part::Part>, MemoryError> {
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

#[tokio::test]
async fn write_blocked_until_read() {
    let dir = TempDir::new().unwrap();
    let target = dir.path().join("foo.txt");
    tokio::fs::write(&target, b"original").await.unwrap();
    let history = ReadHistory::new();
    let c = ctx(dir.path(), PermissionMode::WorkspaceWrite, history.clone());

    let res = WriteTool
        .run(
            c.clone(),
            WriteInput {
                path: "foo.txt".into(),
                content: "new".into(),
            },
        )
        .await;
    assert!(matches!(res, Err(ToolError::ReadBeforeWriteRequired(_))));

    // Read it.
    ReadTool
        .run(
            c.clone(),
            ReadInput {
                path: "foo.txt".into(),
                offset: None,
                limit: None,
            },
        )
        .await
        .unwrap();

    // Write succeeds now.
    let ok = WriteTool
        .run(
            c,
            WriteInput {
                path: "foo.txt".into(),
                content: "new".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(ok.kind, "update");
}

#[tokio::test]
async fn edit_blocked_until_read() {
    let dir = TempDir::new().unwrap();
    let target = dir.path().join("a.md");
    tokio::fs::write(&target, b"hello world").await.unwrap();
    let history = ReadHistory::new();
    let c = ctx(dir.path(), PermissionMode::WorkspaceWrite, history.clone());

    let res = EditTool
        .run(
            c.clone(),
            EditInput::Flat {
                path: "a.md".into(),
                find: "world".into(),
                replace: "rust".into(),
                replace_all: false,
            },
        )
        .await;
    assert!(matches!(res, Err(ToolError::ReadBeforeWriteRequired(_))));

    ReadTool
        .run(
            c.clone(),
            ReadInput {
                path: "a.md".into(),
                offset: None,
                limit: None,
            },
        )
        .await
        .unwrap();

    let ok = EditTool
        .run(
            c,
            EditInput::Flat {
                path: "a.md".into(),
                find: "world".into(),
                replace: "rust".into(),
                replace_all: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(ok.replacements, 1);
    let after = tokio::fs::read_to_string(&target).await.unwrap();
    assert_eq!(after, "hello rust");
}

#[tokio::test]
async fn danger_mode_bypasses_read_gate() {
    let dir = TempDir::new().unwrap();
    let target = dir.path().join("p.txt");
    tokio::fs::write(&target, b"x").await.unwrap();
    let c = ctx(dir.path(), PermissionMode::Danger, ReadHistory::new());
    let ok = WriteTool
        .run(
            c,
            WriteInput {
                path: "p.txt".into(),
                content: "yyy".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(ok.bytes_written, 3);
}

#[tokio::test]
async fn write_rejects_path_escape() {
    let dir = TempDir::new().unwrap();
    let c = ctx(dir.path(), PermissionMode::Danger, ReadHistory::new());
    let res = WriteTool
        .run(
            c,
            WriteInput {
                path: "../escape.txt".into(),
                content: "x".into(),
            },
        )
        .await;
    assert!(matches!(res, Err(ToolError::PathOutsideWorkspace(_))));
}

#[tokio::test]
async fn edit_rejects_ambiguous_match() {
    let dir = TempDir::new().unwrap();
    let target = dir.path().join("dup.txt");
    tokio::fs::write(&target, b"foo foo foo").await.unwrap();
    let history = ReadHistory::new();
    let c = ctx(dir.path(), PermissionMode::Danger, history.clone());
    let res = EditTool
        .run(
            c,
            EditInput::Flat {
                path: "dup.txt".into(),
                find: "foo".into(),
                replace: "bar".into(),
                replace_all: false,
            },
        )
        .await;
    assert!(matches!(res, Err(ToolError::InvalidInput(_))));
}

#[tokio::test]
async fn edit_batch_applies_ops_in_order_single_write() {
    use openlet_core::tools::builtins::edit::{EditOp, EditsInput};

    let dir = TempDir::new().unwrap();
    let target = dir.path().join("b.txt");
    tokio::fs::write(&target, b"alpha beta").await.unwrap();
    let c = ctx(dir.path(), PermissionMode::Danger, ReadHistory::new());

    let ok = EditTool
        .run(
            c,
            EditInput::Batch {
                path: "b.txt".into(),
                edits: EditsInput::Many(vec![
                    EditOp {
                        find: "alpha".into(),
                        replace: "gamma".into(),
                        replace_all: false,
                    },
                    EditOp {
                        find: "beta".into(),
                        replace: "delta".into(),
                        replace_all: false,
                    },
                ]),
            },
        )
        .await
        .unwrap();
    assert_eq!(ok.replacements, 2);
    let after = tokio::fs::read_to_string(&target).await.unwrap();
    assert_eq!(after, "gamma delta");
}

#[tokio::test]
async fn edit_batch_mid_batch_failure_leaves_file_untouched() {
    use openlet_core::tools::builtins::edit::{EditOp, EditsInput};

    let dir = TempDir::new().unwrap();
    let target = dir.path().join("atomic.txt");
    tokio::fs::write(&target, b"one two two").await.unwrap();
    let c = ctx(dir.path(), PermissionMode::Danger, ReadHistory::new());

    // op1 is valid; op2 is ambiguous (>1 match) → whole call aborts, no write.
    let res = EditTool
        .run(
            c,
            EditInput::Batch {
                path: "atomic.txt".into(),
                edits: EditsInput::Many(vec![
                    EditOp {
                        find: "one".into(),
                        replace: "ONE".into(),
                        replace_all: false,
                    },
                    EditOp {
                        find: "two".into(),
                        replace: "TWO".into(),
                        replace_all: false,
                    },
                ]),
            },
        )
        .await;
    assert!(matches!(res, Err(ToolError::InvalidInput(_))));
    // File on disk is unchanged — op1's ONE was never written.
    let after = tokio::fs::read_to_string(&target).await.unwrap();
    assert_eq!(after, "one two two");
}

#[tokio::test]
async fn edit_batch_single_object_via_json() {
    let dir = TempDir::new().unwrap();
    let target = dir.path().join("j.txt");
    tokio::fs::write(&target, b"foo foo").await.unwrap();
    let c = ctx(dir.path(), PermissionMode::Danger, ReadHistory::new());

    let input: EditInput = serde_json::from_value(serde_json::json!({
        "path": "j.txt",
        "edits": { "find": "foo", "replace": "bar", "replace_all": true }
    }))
    .unwrap();
    let ok = EditTool.run(c, input).await.unwrap();
    assert_eq!(ok.replacements, 2);
    let after = tokio::fs::read_to_string(&target).await.unwrap();
    assert_eq!(after, "bar bar");
}

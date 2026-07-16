//! `todo` tool — publishes `todo.updated` after a confirmed persist and
//! reports advisory counts. Uses a local `ToolCtx` wired to a recording
//! event sink + in-memory artifact store so the published event and the
//! persisted bytes are both observable.

mod common;

use std::sync::Arc;

use common::mock_artifact::MemArtifactStore;
use common::mock_event_sink::RecordingEventSink;
use common::mock_memory::MockMemoryStore;
use common::mock_permission::AllowAll;

use openlet_adapters::localfs::LocalFilesystem;
use openlet_core::adapters::tool_executor::ToolCtx;
use openlet_core::agent::AgentRegistry;
use openlet_core::runtime::question_registry::QuestionRegistry;
use openlet_core::runtime::subagent::TaskRegistry;
use openlet_core::tools::ReadHistory;
use openlet_core::tools::Tool;
use openlet_core::tools::builtins::TodoTool;
use openlet_core::tools::builtins::todo::{TodoInput, TodoItem, TodoPriority, TodoStatus};
use openlet_core::types::agent::AgentId;
use openlet_core::types::event::AgentEvent;
use openlet_core::types::message::MessageId;
use openlet_core::types::permission::PermissionMode;
use openlet_core::types::session::SessionId;
use tokio_util::sync::CancellationToken;

/// Build a `ToolCtx` holding the caller's recording sink + artifact store so
/// the test can inspect both the published event and the persisted artifact.
fn ctx_with(
    session_id: SessionId,
    events: Arc<RecordingEventSink>,
    artifacts: Arc<MemArtifactStore>,
) -> ToolCtx {
    let workspace = tempfile::tempdir().expect("tempdir").keep();
    ToolCtx {
        session_id,
        agent_id: AgentId::new(),
        message_id: MessageId::new(),
        call_id: "call-test".into(),
        mode: PermissionMode::Danger,
        fs: Arc::new(LocalFilesystem::new(workspace)),
        permission: Arc::new(AllowAll),
        events,
        artifacts,
        read_history: ReadHistory::new(),
        cancel: CancellationToken::new(),
        questions: Arc::new(QuestionRegistry::new()),
        memory: Arc::new(MockMemoryStore::new()),
        task_registry: Arc::new(TaskRegistry::new(32)),
        agent_registry: Arc::new(AgentRegistry::new()),
    }
}

#[tokio::test]
async fn publishes_todo_updated_after_persist() {
    let session = SessionId::new();
    let events = Arc::new(RecordingEventSink::new());
    let artifacts = Arc::new(MemArtifactStore::new());
    let c = ctx_with(session, events.clone(), artifacts.clone());

    let out = TodoTool
        .run(
            c,
            TodoInput {
                todos: vec![
                    TodoItem {
                        content: "scaffold".into(),
                        status: TodoStatus::InProgress,
                        priority: TodoPriority::High,
                    },
                    TodoItem {
                        content: "tests".into(),
                        status: TodoStatus::Pending,
                        priority: TodoPriority::Medium,
                    },
                ],
            },
        )
        .await
        .unwrap();

    assert_eq!(out.count, 2);
    assert_eq!(out.incomplete, 2);
    assert_eq!(out.in_progress, 1);

    // The artifact landed.
    assert_eq!(artifacts.count(), 1);

    // Exactly one `todo.updated` event carrying both items with wire strings.
    let captured = events.take();
    let todo_events: Vec<_> = captured
        .into_iter()
        .filter_map(|(ev, _)| match ev {
            AgentEvent::TodoUpdated { session_id, items } => Some((session_id, items)),
            _ => None,
        })
        .collect();
    assert_eq!(todo_events.len(), 1);
    let (sid, items) = &todo_events[0];
    assert_eq!(*sid, session);
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].content, "scaffold");
    assert_eq!(items[0].status, "in_progress");
    assert_eq!(items[0].priority, "high");
    assert_eq!(items[1].status, "pending");
}

//! Regression: a `Decision::Pending` permission MUST publish
//! `AgentEvent::PermissionAsked` so a frontend can render a prompt and a
//! human can reply.
//!
//! Without the event, the dispatcher parks on the deferred forever — the
//! turn loop hangs, the session stays `running`, and a re-prompt 409s.
//! This is the exact failure that the mock-only suite missed: scripted
//! providers terminate cleanly, so the silent-Pending hang only appears
//! when a real model requests a tool whose permission falls through to
//! the mode default (`WorkspaceWrite` → `Ask`).
//!
//! The test drives the REAL `ConfigPermissionMgr` (not a mock gate) so
//! the deferred + ask_id plumbing is genuine, then asserts:
//!   1. dispatch publishes `PermissionAsked { ask_id, request }` BEFORE
//!      it parks,
//!   2. replying to that `ask_id` unparks the tool and it actually runs.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::mock_event_sink::RecordingEventSink;
use common::mock_memory::MockMemoryStore;
use common::mock_tool::{NoopTool, make_registry};
use leti_adapters::config_perm::ConfigPermissionMgr;
use leti_adapters::localfs::LocalFilesystem;
use leti_core::adapters::artifact_store::ArtifactStore;
use leti_core::adapters::event_sink::EventSink;
use leti_core::adapters::memory_store::MemoryStore;
use leti_core::adapters::permission_manager::PermissionManager;
use leti_core::adapters::tool_executor::ToolCtx;
use leti_core::dispatch::HookChains;
use leti_core::tools::{ReadHistory, ToolDispatchResult, ToolInvocation, dispatch_batch};
use leti_core::types::agent::AgentId;
use leti_core::types::event::AgentEvent;
use leti_core::types::message::MessageId;
use leti_core::types::permission::{AskId, Decision, PermissionCtx, PermissionMode};
use leti_core::types::session::SessionId;
use serde_json::json;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

mod mock_artifact_local {
    pub use crate::common::mock_artifact::MemArtifactStore;
}

struct PendingDispatch {
    permission: Arc<ConfigPermissionMgr>,
    recording: Arc<RecordingEventSink>,
    cancel: CancellationToken,
    handle: tokio::task::JoinHandle<Vec<ToolDispatchResult>>,
}

fn spawn_pending_dispatch(call_count: usize) -> PendingDispatch {
    let session_id = SessionId::new();
    let registry = make_registry(vec![Arc::new(NoopTool::new("writer", false))]);
    let permission_impl = Arc::new(ConfigPermissionMgr::new());
    let permission: Arc<dyn PermissionManager> =
        Arc::clone(&permission_impl) as Arc<dyn PermissionManager>;
    let recording = Arc::new(RecordingEventSink::new());
    let events: Arc<dyn EventSink> = Arc::clone(&recording) as Arc<dyn EventSink>;

    let artifacts: Arc<dyn ArtifactStore> = Arc::new(mock_artifact_local::MemArtifactStore::new());
    let memory: Arc<dyn MemoryStore> = Arc::new(MockMemoryStore::new());
    let dir = TempDir::new().unwrap();
    let fs: Arc<dyn leti_core::adapters::filesystem::Filesystem> =
        Arc::new(LocalFilesystem::new(dir.path().to_path_buf()));
    let hook_chains = Arc::new(HookChains::new());
    let cancel = CancellationToken::new();

    let perm_ctx = PermissionCtx {
        session_id,
        mode: PermissionMode::WorkspaceWrite,
        interaction_mode: Default::default(),
        ext: Default::default(),
    };

    let ctx_for = {
        let fs = Arc::clone(&fs);
        let permission = Arc::clone(&permission);
        let events = Arc::clone(&events);
        let artifacts = Arc::clone(&artifacts);
        let memory = Arc::clone(&memory);
        let cancel = cancel.clone();
        move |inv: &ToolInvocation| ToolCtx {
            session_id,
            agent_id: AgentId::new(),
            message_id: MessageId::new(),
            call_id: inv.call_id.clone(),
            ext: Default::default(),
            fs: Arc::clone(&fs),
            mode: PermissionMode::WorkspaceWrite,
            permission: Arc::clone(&permission),
            events: Arc::clone(&events),
            artifacts: Arc::clone(&artifacts),
            read_history: ReadHistory::new(),
            cancel: cancel.clone(),
            questions: Arc::new(leti_core::runtime::QuestionRegistry::new()),
            memory: Arc::clone(&memory),
            task_registry: Arc::new(leti_core::runtime::subagent::TaskRegistry::new(32)),
            agent_registry: Arc::new(leti_core::agent::AgentRegistry::new()),
        }
    };

    let invocations = (0..call_count)
        .map(|i| ToolInvocation {
            call_id: format!("c{}", i + 1),
            name: "writer".into(),
            args: json!({}),
        })
        .collect();

    let handle = {
        let registry = Arc::clone(&registry);
        let permission = Arc::clone(&permission);
        let hook_chains = Arc::clone(&hook_chains);
        let events = Arc::clone(&events);
        tokio::spawn(async move {
            dispatch_batch(
                &registry,
                &permission,
                &hook_chains,
                &events,
                session_id,
                ctx_for,
                perm_ctx,
                invocations,
            )
            .await
        })
    };

    PendingDispatch {
        permission: permission_impl,
        recording,
        cancel,
        handle,
    }
}

async fn wait_for_ask(recording: &RecordingEventSink, except: Option<AskId>) -> AskId {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(id) = recording.snapshot().iter().find_map(|(ev, _)| match ev {
                AgentEvent::PermissionAsked { ask_id, .. } if Some(*ask_id) != except => {
                    Some(*ask_id)
                }
                _ => None,
            }) {
                break id;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("PermissionAsked must be published before the deferred is awaited")
}

#[tokio::test]
async fn pending_permission_publishes_ask_then_reply_unparks_tool() {
    let dispatch = spawn_pending_dispatch(1);
    let ask_id = wait_for_ask(&dispatch.recording, None).await;

    dispatch
        .permission
        .reply(ask_id, Decision::Allow)
        .await
        .expect("reply to the announced ask");

    let results = tokio::time::timeout(Duration::from_secs(2), dispatch.handle)
        .await
        .expect("dispatch must complete once the ask is answered")
        .expect("dispatch task join");

    assert_eq!(results.len(), 1);
    assert!(
        results[0].outcome.is_ok(),
        "tool must run after the ask is allowed: {:?}",
        results[0].outcome
    );
}

#[tokio::test]
async fn cancelling_pending_permission_drains_ask_and_publishes_resolution() {
    let dispatch = spawn_pending_dispatch(1);
    let ask_id = wait_for_ask(&dispatch.recording, None).await;
    dispatch.cancel.cancel();

    let results = tokio::time::timeout(Duration::from_secs(2), dispatch.handle)
        .await
        .expect("dispatch must complete after cancellation")
        .expect("dispatch task join");

    assert_eq!(results.len(), 1);
    assert!(
        matches!(
            results[0].outcome,
            Err(leti_core::error::ToolError::Cancelled)
        ),
        "cancelled pending tool must be terminal: {:?}",
        results[0].outcome
    );
    assert_eq!(
        dispatch.permission.pending_count(),
        0,
        "cancel must drain the pending ask"
    );
    assert!(
        dispatch.recording.snapshot().iter().any(|(ev, _)| matches!(
            ev,
            AgentEvent::PermissionResolved { ask_id: resolved, .. } if *resolved == ask_id
        )),
        "cancel must publish PermissionResolved for the pending ask"
    );
}

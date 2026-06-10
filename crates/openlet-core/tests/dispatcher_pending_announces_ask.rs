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
use openlet_adapters::config_perm::ConfigPermissionMgr;
use openlet_adapters::localfs::LocalFilesystem;
use openlet_core::adapters::artifact_store::ArtifactStore;
use openlet_core::adapters::event_sink::EventSink;
use openlet_core::adapters::memory_store::MemoryStore;
use openlet_core::adapters::permission_manager::PermissionManager;
use openlet_core::adapters::tool_executor::ToolCtx;
use openlet_core::dispatch::HookChains;
use openlet_core::tools::{ReadHistory, ToolInvocation, dispatch_batch};
use openlet_core::types::agent::AgentId;
use openlet_core::types::event::AgentEvent;
use openlet_core::types::message::MessageId;
use openlet_core::types::permission::{AskId, Decision, PermissionCtx, PermissionMode};
use openlet_core::types::session::SessionId;
use serde_json::json;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

mod mock_artifact_local {
    pub use crate::common::mock_artifact::MemArtifactStore;
}

fn find_ask(
    events: &[(AgentEvent, openlet_core::adapters::event_sink::Persistence)],
) -> Option<AskId> {
    events.iter().find_map(|(ev, _)| match ev {
        AgentEvent::PermissionAsked { ask_id, .. } => Some(*ask_id),
        _ => None,
    })
}

#[tokio::test]
async fn pending_permission_publishes_ask_then_reply_unparks_tool() {
    let session_id = SessionId::new();
    let registry = make_registry(vec![Arc::new(NoopTool::new("writer", false))]);

    // Real gate, no rules → WorkspaceWrite mode falls through to Ask for
    // any unmatched permission. This is precisely the production default
    // when a user runs the agent with `--mode workspace-write` and the
    // model requests a tool no rule pre-approves.
    let permission: Arc<dyn PermissionManager> = Arc::new(ConfigPermissionMgr::new());
    let recording = Arc::new(RecordingEventSink::new());
    let events: Arc<dyn EventSink> = Arc::clone(&recording) as Arc<dyn EventSink>;

    let artifacts: Arc<dyn ArtifactStore> = Arc::new(mock_artifact_local::MemArtifactStore::new());
    let memory: Arc<dyn MemoryStore> = Arc::new(MockMemoryStore::new());
    let dir = TempDir::new().unwrap();
    let fs: Arc<dyn openlet_core::adapters::filesystem::Filesystem> =
        Arc::new(LocalFilesystem::new(dir.path().to_path_buf()));
    let hook_chains = Arc::new(HookChains::new());
    let cancel = CancellationToken::new();

    let perm_ctx = PermissionCtx {
        session_id,
        mode: PermissionMode::WorkspaceWrite,
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
            fs: Arc::clone(&fs),
            mode: PermissionMode::WorkspaceWrite,
            permission: Arc::clone(&permission),
            events: Arc::clone(&events),
            artifacts: Arc::clone(&artifacts),
            read_history: ReadHistory::new(),
            cancel: cancel.clone(),
            questions: Arc::new(openlet_core::runtime::QuestionRegistry::new()),
            memory: Arc::clone(&memory),
            task_registry: Arc::new(openlet_core::runtime::subagent::TaskRegistry::new(32)),
            agent_registry: Arc::new(openlet_core::agent::AgentRegistry::new()),
        }
    };

    let invocations = vec![ToolInvocation {
        call_id: "c1".into(),
        name: "writer".into(),
        args: json!({}),
    }];

    // Spawn dispatch — it WILL park on the deferred until we reply.
    let dispatch_handle = {
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

    // Poll for the PermissionAsked event. Before the fix this never
    // arrives and the dispatch future is parked forever, so the timeout
    // is the regression guard: if we can't find the ask in 2s, the bug
    // is back.
    let ask_id = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(id) = find_ask(&recording.snapshot()) {
                break id;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("PermissionAsked must be published before the deferred is awaited");

    // Reply allow — this resolves the deferred and unparks the tool.
    permission
        .reply(ask_id, Decision::Allow)
        .await
        .expect("reply to the announced ask");

    let results = tokio::time::timeout(Duration::from_secs(2), dispatch_handle)
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

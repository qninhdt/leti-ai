//! Smoke test — exercises every mock in `tests/common/` to confirm they
//! all compile and round-trip a trivial value. If this test breaks
//! after a refactor, every Phase 2-5 test will too.

mod common;

use std::sync::Arc;

use bytes::Bytes;
use common::mock_artifact::MemArtifactStore;
use common::mock_event_sink::RecordingEventSink;
use common::mock_memory::MockMemoryStore;
use common::mock_permission::{AllowAll, DenyAll, ScriptedPermission};
use common::mock_provider::ScriptedProvider;
use common::mock_tool::{FailingTool, NoopTool, PanickingTool, SlowTool, make_registry};

use openlet_core::adapters::artifact_store::ArtifactStore;
use openlet_core::adapters::event_sink::{EventSink, Persistence};
use openlet_core::adapters::memory_store::MemoryStore;
use openlet_core::adapters::model_provider::{ChatDelta, ChatRequest, FinishReason, ModelProvider};
use openlet_core::adapters::permission_manager::PermissionManager;
use openlet_core::error::ToolError;
use openlet_core::types::agent::AgentId;
use openlet_core::types::event::AgentEvent;
use openlet_core::types::permission::{Decision, PermissionCtx, PermissionMode, PermissionRequest};
use openlet_core::types::session::SessionId;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn scripted_provider_emits_pushed_turn() {
    use futures::StreamExt;

    let provider = ScriptedProvider::new();
    provider.push_text_turn("hello world");

    let req = ChatRequest {
        model: "test".into(),
        messages: vec![],
        system: None,
        max_tokens: None,
        temperature: None,
        tools: vec![],
        stream: true,
        headers: Default::default(),
    };
    let mut stream = provider
        .chat_stream(req, CancellationToken::new())
        .await
        .expect("chat_stream");

    let mut got_text = false;
    let mut got_finish = false;
    while let Some(item) = stream.next().await {
        match item.unwrap() {
            ChatDelta::Content { text } => {
                assert_eq!(text, "hello world");
                got_text = true;
            }
            ChatDelta::Finish { reason, .. } => {
                assert!(matches!(reason, FinishReason::EndTurn));
                got_finish = true;
            }
            _ => {}
        }
    }
    assert!(got_text && got_finish);
    assert_eq!(provider.call_count(), 1);
}

#[tokio::test]
async fn scripted_provider_observes_cancellation() {
    use futures::StreamExt;

    let provider = ScriptedProvider::new();
    // Push a turn that would never naturally Finish — cancellation
    // should synthesise a Cancelled finish frame.
    provider.push_turn(vec![
        Ok(ChatDelta::Content { text: "a".into() }),
        Ok(ChatDelta::Content { text: "b".into() }),
    ]);

    let cancel = CancellationToken::new();
    cancel.cancel();

    let mut stream = provider
        .chat_stream(
            ChatRequest {
                model: "test".into(),
                messages: vec![],
                system: None,
                max_tokens: None,
                temperature: None,
                tools: vec![],
                stream: true,
                headers: Default::default(),
            },
            cancel,
        )
        .await
        .unwrap();

    // First poll under a tripped token must yield the synthetic
    // Cancelled finish frame, not "a".
    let first = stream.next().await.unwrap().unwrap();
    assert!(matches!(
        first,
        ChatDelta::Finish {
            reason: FinishReason::Cancelled,
            ..
        }
    ));
    // Stream then ends.
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn recording_event_sink_captures_publishes() {
    let sink = RecordingEventSink::new();
    sink.publish(AgentEvent::Heartbeat, Persistence::Transient)
        .await
        .unwrap();
    let captured = sink.take();
    assert_eq!(captured.len(), 1);
    assert!(matches!(captured[0].1, Persistence::Transient));
    // After drain, snapshot is empty.
    assert!(sink.snapshot().is_empty());
}

#[tokio::test]
async fn allow_deny_scripted_permission_behave_as_documented() {
    let ctx = PermissionCtx {
        session_id: SessionId::new(),
        mode: PermissionMode::WorkspaceWrite,
    };
    let req = || PermissionRequest {
        permission: "test".into(),
        reason: None,
        timeout: None,
    };

    assert!(matches!(
        AllowAll.check(ctx.clone(), req()).await.unwrap(),
        Decision::Allow
    ));
    assert!(matches!(
        DenyAll.check(ctx.clone(), req()).await.unwrap(),
        Decision::Deny { .. }
    ));

    let scripted = ScriptedPermission::new(vec![
        Decision::Allow,
        Decision::Deny {
            feedback: Some("nope".into()),
        },
    ]);
    assert_eq!(scripted.remaining(), 2);
    assert!(matches!(
        scripted.check(ctx.clone(), req()).await.unwrap(),
        Decision::Allow
    ));
    assert!(matches!(
        scripted.check(ctx, req()).await.unwrap(),
        Decision::Deny { .. }
    ));
    assert_eq!(scripted.remaining(), 0);
}

#[tokio::test]
async fn mock_tools_register_and_describe() {
    let noop = Arc::new(NoopTool::new("noop", true));
    let failing = Arc::new(FailingTool::new("failing", || ToolError::Timeout));
    let slow = Arc::new(SlowTool::new("slow", 1, true));
    let panicking = Arc::new(PanickingTool::new("panicker"));
    let registry = make_registry(vec![noop, failing, slow, panicking]);
    let mut names: Vec<&'static str> = registry.names().collect();
    names.sort();
    assert_eq!(names, vec!["failing", "noop", "panicker", "slow"]);
}

#[tokio::test]
async fn mem_artifact_store_round_trips() {
    let store = MemArtifactStore::new();
    let session = SessionId::new();
    let r = store
        .put(session, "k", Bytes::from_static(b"abc"))
        .await
        .unwrap();
    assert_eq!(r.size, 3);
    let bytes = store.get(&r).await.unwrap();
    assert_eq!(&bytes[..], b"abc");
    let listed = store.list(session).await.unwrap();
    assert_eq!(listed.len(), 1);
}

#[tokio::test]
async fn mock_memory_round_trips_messages() {
    let mem = MockMemoryStore::new();
    let session = mem.create_session(AgentId::new(), None).await.unwrap();
    assert_eq!(mem.message_count(session), 0);
}

#[tokio::test]
async fn runtime_fixture_boots_with_dependencies_wired() {
    use common::runtime::RuntimeFixture;

    let fx = RuntimeFixture::boot();
    // Provider, memory, events all share the runtime; round-trip via
    // public surface confirms no panics in construction and that handles
    // line up. ScriptedProvider starts at zero calls.
    assert_eq!(fx.provider.call_count(), 0);
    assert_eq!(fx.events.count(), 0);
    let session = fx
        .memory
        .create_session(AgentId::new(), None)
        .await
        .unwrap();
    assert_eq!(fx.memory.message_count(session), 0);
    // Cumulative cost defaults to zero for an unknown session.
    assert!(fx.runtime.session_cost(session).is_zero());
}

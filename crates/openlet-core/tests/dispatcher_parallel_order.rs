//! `dispatch_batch` order preservation + panic isolation.
//!
//! Two invariants under test:
//!
//! 1. **Order preservation**: when invocations interleave parallel-safe
//!    and non-safe tools (e.g. positions 0,2 safe; 1,3 not), the
//!    returned `ToolDispatchResult` order MUST equal the invocation
//!    order, regardless of how the partition was scheduled.
//! 2. **Panic isolation**: a panicking tool inside the batch MUST
//!    surface as `ToolError::Io("tool 'X' panicked")`, the panic must
//!    not unwind the dispatcher, and other invocations must complete.

mod common;

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use common::mock_event_sink::RecordingEventSink;
use common::mock_permission::AllowAll;
use common::mock_tool::{NoopTool, PanickingTool, SlowTool, make_registry};
use openlet_adapters::localfs::LocalFilesystem;
use openlet_core::adapters::artifact_store::{ArtifactRef, ArtifactStore};
use openlet_core::adapters::event_sink::EventSink;
use openlet_core::adapters::memory_store::MemoryStore;
use openlet_core::adapters::permission_manager::PermissionManager;
use openlet_core::adapters::tool_executor::ToolCtx;
use openlet_core::dispatch::HookChains;
use openlet_core::error::{ArtifactError, ToolError};
use openlet_core::tools::{ReadHistory, ToolInvocation, dispatch_batch};
use openlet_core::types::agent::AgentId;
use openlet_core::types::message::MessageId;
use openlet_core::types::permission::{PermissionCtx, PermissionMode};
use openlet_core::types::session::SessionId;
use serde_json::json;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct DiscardArtifacts;

#[async_trait]
impl ArtifactStore for DiscardArtifacts {
    async fn put(
        &self,
        session: SessionId,
        key: &str,
        bytes: Bytes,
    ) -> Result<ArtifactRef, ArtifactError> {
        Ok(ArtifactRef {
            session_id: session,
            key: key.to_string(),
            size: bytes.len() as u64,
            mime: None,
        })
    }
    async fn get(&self, _r: &ArtifactRef) -> Result<Bytes, ArtifactError> {
        Err(ArtifactError::NotFound("discard".into()))
    }
    async fn list(&self, _session: SessionId) -> Result<Vec<ArtifactRef>, ArtifactError> {
        Ok(vec![])
    }
}

fn make_ctx(
    workspace: &Path,
    permission: Arc<dyn PermissionManager>,
    events: Arc<dyn EventSink>,
    memory: Arc<dyn MemoryStore>,
    call_id: &str,
) -> ToolCtx {
    ToolCtx {
        session_id: SessionId::new(),
        agent_id: AgentId::new(),
        message_id: MessageId::new(),
        call_id: call_id.to_string(),
        fs: Arc::new(LocalFilesystem::new(workspace.to_path_buf())),
        mode: PermissionMode::Danger,
        permission,
        events,
        artifacts: Arc::new(DiscardArtifacts),
        read_history: ReadHistory::new(),
        cancel: CancellationToken::new(),
        questions: Arc::new(openlet_core::runtime::QuestionRegistry::new()),
        memory,
        task_registry: Arc::new(openlet_core::runtime::subagent::TaskRegistry::new(32)),
        agent_registry: Arc::new(openlet_core::agent::AgentRegistry::new()),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn order_preserved_with_mixed_parallel_serial_partition() {
    const ITERS: usize = 50;

    // 4 tools — alternating parallel-safe (0, 2) and serial (1, 3).
    // The slow ones widen the race window so order preservation must
    // come from the dispatcher, not luck.
    let tools = vec![
        Arc::new(SlowTool::new("safe_a", 5, true)) as _,
        Arc::new(SlowTool::new("serial_b", 1, false)) as _,
        Arc::new(SlowTool::new("safe_c", 5, true)) as _,
        Arc::new(SlowTool::new("serial_d", 1, false)) as _,
    ];
    let registry = make_registry(tools);
    let permission: Arc<dyn PermissionManager> = Arc::new(AllowAll);
    let events: Arc<dyn EventSink> = Arc::new(RecordingEventSink::new());
    let memory: Arc<dyn MemoryStore> = Arc::new(common::mock_memory::MockMemoryStore::new());
    let dir = TempDir::new().unwrap();
    let workspace = dir.path().to_path_buf();
    let hook_chains = Arc::new(HookChains::new());

    for _ in 0..ITERS {
        let invocations = vec![
            ToolInvocation {
                call_id: "call-0".into(),
                name: "safe_a".into(),
                args: json!({}),
            },
            ToolInvocation {
                call_id: "call-1".into(),
                name: "serial_b".into(),
                args: json!({}),
            },
            ToolInvocation {
                call_id: "call-2".into(),
                name: "safe_c".into(),
                args: json!({}),
            },
            ToolInvocation {
                call_id: "call-3".into(),
                name: "serial_d".into(),
                args: json!({}),
            },
        ];

        let perm_ctx = PermissionCtx {
            session_id: SessionId::new(),
            mode: PermissionMode::Danger,
        };

        let permission_for_ctx = Arc::clone(&permission);
        let events_for_ctx = Arc::clone(&events);
        let memory_for_ctx = Arc::clone(&memory);
        let workspace_for_ctx = workspace.clone();
        let ctx_for = move |inv: &ToolInvocation| {
            make_ctx(
                &workspace_for_ctx,
                Arc::clone(&permission_for_ctx),
                Arc::clone(&events_for_ctx),
                Arc::clone(&memory_for_ctx),
                &inv.call_id,
            )
        };

        let results = dispatch_batch(
            &registry,
            &permission,
            &hook_chains,
            &events,
            perm_ctx.session_id,
            ctx_for,
            perm_ctx,
            invocations,
        )
        .await;

        let ids: Vec<&str> = results.iter().map(|r| r.call_id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["call-0", "call-1", "call-2", "call-3"],
            "dispatcher must preserve invocation order"
        );
        for r in &results {
            assert!(r.outcome.is_ok(), "{} failed: {:?}", r.call_id, r.outcome);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn panicking_tool_surfaces_io_error_others_complete() {
    let counting_noop = Arc::new(NoopTool::new("noop_0", true));

    let tools = vec![
        counting_noop.clone() as _,
        Arc::new(NoopTool::new("noop_1", true)) as _,
        Arc::new(PanickingTool::new("panicker")) as _,
        Arc::new(NoopTool::new("noop_3", true)) as _,
    ];
    let registry = make_registry(tools);
    let permission: Arc<dyn PermissionManager> = Arc::new(AllowAll);
    let events: Arc<dyn EventSink> = Arc::new(RecordingEventSink::new());
    let memory: Arc<dyn MemoryStore> = Arc::new(common::mock_memory::MockMemoryStore::new());
    let dir = TempDir::new().unwrap();
    let workspace = dir.path().to_path_buf();
    let hook_chains = Arc::new(HookChains::new());

    let invocations = vec![
        ToolInvocation {
            call_id: "0".into(),
            name: "noop_0".into(),
            args: json!({}),
        },
        ToolInvocation {
            call_id: "1".into(),
            name: "noop_1".into(),
            args: json!({}),
        },
        ToolInvocation {
            call_id: "2".into(),
            name: "panicker".into(),
            args: json!({}),
        },
        ToolInvocation {
            call_id: "3".into(),
            name: "noop_3".into(),
            args: json!({}),
        },
    ];

    let perm_ctx = PermissionCtx {
        session_id: SessionId::new(),
        mode: PermissionMode::Danger,
    };

    let permission_for_ctx = Arc::clone(&permission);
    let events_for_ctx = Arc::clone(&events);
    let memory_for_ctx = Arc::clone(&memory);
    let workspace_for_ctx = workspace.clone();
    let ctx_for = move |inv: &ToolInvocation| {
        make_ctx(
            &workspace_for_ctx,
            Arc::clone(&permission_for_ctx),
            Arc::clone(&events_for_ctx),
            Arc::clone(&memory_for_ctx),
            &inv.call_id,
        )
    };

    let results = dispatch_batch(
        &registry,
        &permission,
        &hook_chains,
        &events,
        perm_ctx.session_id,
        ctx_for,
        perm_ctx,
        invocations,
    )
    .await;

    assert_eq!(results.len(), 4);
    let ids: Vec<&str> = results.iter().map(|r| r.call_id.as_str()).collect();
    assert_eq!(ids, vec!["0", "1", "2", "3"]);
    assert!(results[0].outcome.is_ok(), "noop_0 ok");
    assert!(results[1].outcome.is_ok(), "noop_1 ok");
    let panic_err = results[2]
        .outcome
        .as_ref()
        .expect_err("panicker must error");
    let msg = format!("{panic_err}");
    assert!(
        matches!(panic_err, ToolError::Io(_)) && msg.contains("panicked"),
        "panic must surface as ToolError::Io with 'panicked' in the message; got: {msg}"
    );
    assert!(results[3].outcome.is_ok(), "noop_3 ok after panic");

    // counting_noop ran exactly once.
    assert_eq!(counting_noop.run_count(), 1);
}

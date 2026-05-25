//! Dispatcher tests — parallel-safe partition + permission decisions.
//!
//! Builds a synthetic registry where a `slow_read` tool blocks for a
//! known duration before returning. With `parallel_safe = true`, three
//! `slow_read`s should overlap; the wallclock is bounded well under
//! `3 * single_call_duration`. A `slow_write` (parallel_safe = false)
//! interleaved in the batch must run after the safe set.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bytes::Bytes;
use openlet_adapters::localfs::LocalFilesystem;
use openlet_core::adapters::artifact_store::{ArtifactRef, ArtifactStore};
use openlet_core::adapters::event_sink::{EventSink, Persistence};
use openlet_core::adapters::permission_manager::PermissionManager;
use openlet_core::adapters::tool_executor::ToolCtx;
use openlet_core::dispatch::{HookChains, HookEntry};
use openlet_core::error::{ArtifactError, EventError, PermissionError, ToolError};
use openlet_core::hooks::{
    HookKind, HookResult, Priority,
    io::{AfterToolCallCtx, BeforeToolCallCtx},
};
use openlet_core::tools::{
    ReadHistory, Tool, ToolDispatchResult, ToolInvocation, ToolRegistry, dispatch_batch,
};
use openlet_core::types::agent::AgentId;
use openlet_core::types::event::{AgentEvent, EventFilter};
use openlet_core::types::message::MessageId;
use openlet_core::types::permission::{
    AlwaysScope, AskId, Decision, PermissionCtx, PermissionMode, PermissionRequest, PermissionRule,
};
use openlet_core::types::session::SessionId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
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
    async fn accept_ask(&self, _: AskId, _: AlwaysScope) -> Result<(), PermissionError> {
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

#[derive(Default)]
struct NoopMemory;

#[async_trait]
impl openlet_core::adapters::memory_store::MemoryStore for NoopMemory {
    async fn create_session(
        &self,
        _: AgentId,
        _: Option<SessionId>,
    ) -> Result<SessionId, openlet_core::error::MemoryError> {
        Err(openlet_core::error::MemoryError::Unimplemented)
    }
    async fn get_session(
        &self,
        _: SessionId,
    ) -> Result<Option<openlet_core::types::session::SessionMeta>, openlet_core::error::MemoryError>
    {
        Ok(None)
    }
    async fn list_sessions(
        &self,
        _: openlet_core::types::session::SessionFilter,
    ) -> Result<Vec<openlet_core::types::session::SessionMeta>, openlet_core::error::MemoryError>
    {
        Ok(vec![])
    }
    async fn update_status(
        &self,
        _: SessionId,
        _: openlet_core::types::session::SessionStatus,
        _: &str,
    ) -> Result<(), openlet_core::error::MemoryError> {
        Ok(())
    }
    async fn switch_agent(
        &self,
        _: SessionId,
        _: &str,
    ) -> Result<(), openlet_core::error::MemoryError> {
        Ok(())
    }
    async fn update_permission_mode(
        &self,
        _: SessionId,
        _: PermissionMode,
    ) -> Result<(), openlet_core::error::MemoryError> {
        Ok(())
    }
    async fn update_session_extensions(
        &self,
        _: SessionId,
        _: serde_json::Value,
    ) -> Result<(), openlet_core::error::MemoryError> {
        Ok(())
    }
    async fn delete_session(&self, _: SessionId) -> Result<(), openlet_core::error::MemoryError> {
        Ok(())
    }
    async fn append_message(
        &self,
        _: SessionId,
        msg: openlet_core::types::message::Message,
    ) -> Result<MessageId, openlet_core::error::MemoryError> {
        Ok(msg.id)
    }
    async fn append_part(
        &self,
        _: MessageId,
        _: openlet_core::types::part::Part,
    ) -> Result<openlet_core::types::part::PartId, openlet_core::error::MemoryError> {
        Ok(openlet_core::types::part::PartId::new())
    }
    async fn upsert_part(
        &self,
        _: MessageId,
        _: openlet_core::types::part::PartId,
        _: openlet_core::types::part::Part,
    ) -> Result<(), openlet_core::error::MemoryError> {
        Ok(())
    }
    async fn list_messages(
        &self,
        _: SessionId,
    ) -> Result<Vec<openlet_core::types::message::Message>, openlet_core::error::MemoryError> {
        Ok(vec![])
    }
    async fn list_parts(
        &self,
        _: SessionId,
        _: MessageId,
    ) -> Result<Vec<openlet_core::types::part::Part>, openlet_core::error::MemoryError> {
        Ok(vec![])
    }
    async fn record_read(
        &self,
        _: SessionId,
        _: std::path::PathBuf,
    ) -> Result<(), openlet_core::error::MemoryError> {
        Ok(())
    }
}

fn ctx(workspace: &Path) -> ToolCtx {
    ToolCtx {
        session_id: SessionId::new(),
        agent_id: AgentId::new(),
        message_id: MessageId::new(),
        call_id: "call".into(),
        fs: Arc::new(LocalFilesystem::new(workspace.to_path_buf())),
        mode: PermissionMode::Danger,
        permission: Arc::new(AllowAll),
        events: Arc::new(NoopBus),
        artifacts: Arc::new(DiscardArtifacts),
        read_history: ReadHistory::new(),
        cancel: CancellationToken::new(),
        questions: Arc::new(openlet_core::runtime::QuestionRegistry::new()),
        memory: Arc::new(NoopMemory),
        task_registry: Arc::new(openlet_core::runtime::subagent::TaskRegistry::new(32)),
        agent_registry: Arc::new(openlet_core::agent::AgentRegistry::new()),
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct Empty {}

#[derive(Debug, Clone, Serialize, JsonSchema)]
struct Tagged {
    tag: String,
    finished_at_ms: u128,
}

/// 100 ms blocking read, parallel-safe.
struct SlowRead {
    name: &'static str,
    started_at: Arc<Instant>,
}

#[async_trait]
impl Tool for SlowRead {
    type Input = Empty;
    type Output = Tagged;

    fn name(&self) -> &'static str {
        self.name
    }
    fn description(&self) -> &'static str {
        "slow parallel-safe read for tests"
    }
    fn parallel_safe(&self) -> bool {
        true
    }
    fn permission(&self, _: &Self::Input) -> PermissionRequest {
        PermissionRequest {
            permission: format!("read:{}", self.name),
            reason: None,
            timeout: None,
        }
    }

    async fn run(&self, _: ToolCtx, _: Self::Input) -> Result<Self::Output, ToolError> {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let finished_at_ms = self.started_at.elapsed().as_millis();
        Ok(Tagged {
            tag: self.name.to_string(),
            finished_at_ms,
        })
    }
}

/// 50 ms blocking write, NOT parallel-safe. Captures order via shared counter.
struct OrderingWrite {
    name: &'static str,
    started_at: Arc<Instant>,
    write_seen_after: Arc<AtomicUsize>,
    safe_finished: Arc<AtomicUsize>,
}

#[async_trait]
impl Tool for OrderingWrite {
    type Input = Empty;
    type Output = Tagged;

    fn name(&self) -> &'static str {
        self.name
    }
    fn description(&self) -> &'static str {
        "non-parallel write for tests"
    }
    fn parallel_safe(&self) -> bool {
        false
    }
    fn permission(&self, _: &Self::Input) -> PermissionRequest {
        PermissionRequest {
            permission: format!("write:{}", self.name),
            reason: None,
            timeout: None,
        }
    }

    async fn run(&self, _: ToolCtx, _: Self::Input) -> Result<Self::Output, ToolError> {
        // Snapshot how many parallel-safe tools had finished by the time
        // this serial tool begins. Test asserts it equals the total safe
        // count, proving the partition runs safe-first then serial.
        self.write_seen_after
            .store(self.safe_finished.load(Ordering::SeqCst), Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(50)).await;
        let finished_at_ms = self.started_at.elapsed().as_millis();
        Ok(Tagged {
            tag: self.name.to_string(),
            finished_at_ms,
        })
    }
}

#[tokio::test]
async fn parallel_safe_reads_overlap_write_runs_after() {
    let started = Arc::new(Instant::now());
    let safe_finished = Arc::new(AtomicUsize::new(0));
    let write_seen_after = Arc::new(AtomicUsize::new(usize::MAX));

    // Wrap SlowRead so we can bump the safe-finished counter on completion.
    struct CountingRead {
        inner: SlowRead,
        counter: Arc<AtomicUsize>,
    }
    #[async_trait]
    impl Tool for CountingRead {
        type Input = Empty;
        type Output = Tagged;
        fn name(&self) -> &'static str {
            self.inner.name()
        }
        fn description(&self) -> &'static str {
            self.inner.description()
        }
        fn parallel_safe(&self) -> bool {
            self.inner.parallel_safe()
        }
        fn permission(&self, i: &Self::Input) -> PermissionRequest {
            self.inner.permission(i)
        }
        async fn run(&self, ctx: ToolCtx, i: Self::Input) -> Result<Self::Output, ToolError> {
            let r = self.inner.run(ctx, i).await;
            self.counter.fetch_add(1, Ordering::SeqCst);
            r
        }
    }

    let registry = ToolRegistry::builder()
        .register(CountingRead {
            inner: SlowRead {
                name: "read_a",
                started_at: Arc::clone(&started),
            },
            counter: Arc::clone(&safe_finished),
        })
        .register(CountingRead {
            inner: SlowRead {
                name: "read_b",
                started_at: Arc::clone(&started),
            },
            counter: Arc::clone(&safe_finished),
        })
        .register(CountingRead {
            inner: SlowRead {
                name: "read_c",
                started_at: Arc::clone(&started),
            },
            counter: Arc::clone(&safe_finished),
        })
        .register(OrderingWrite {
            name: "write_x",
            started_at: Arc::clone(&started),
            write_seen_after: Arc::clone(&write_seen_after),
            safe_finished: Arc::clone(&safe_finished),
        })
        .build();
    let registry = registry;
    let permission: Arc<dyn PermissionManager> = Arc::new(AllowAll);
    let dir = TempDir::new().unwrap();

    let invocations = vec![
        ToolInvocation {
            call_id: "1".into(),
            name: "read_a".into(),
            args: json!({}),
        },
        ToolInvocation {
            call_id: "2".into(),
            name: "write_x".into(),
            args: json!({}),
        },
        ToolInvocation {
            call_id: "3".into(),
            name: "read_b".into(),
            args: json!({}),
        },
        ToolInvocation {
            call_id: "4".into(),
            name: "read_c".into(),
            args: json!({}),
        },
    ];

    let perm_ctx = PermissionCtx {
        session_id: SessionId::new(),
        mode: PermissionMode::Danger,
    };
    let workspace = dir.path().to_path_buf();
    let ctx_for = move |_inv: &ToolInvocation| ctx(&workspace);

    let hook_chains = std::sync::Arc::new(HookChains::new());
    let events: Arc<dyn EventSink> = Arc::new(NoopBus);
    let wallclock_start = Instant::now();
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
    let wallclock = wallclock_start.elapsed();

    // 4 results, returned in input order (call_id 1..4).
    assert_eq!(results.len(), 4);
    let ids: Vec<&str> = results.iter().map(|r| r.call_id.as_str()).collect();
    assert_eq!(ids, vec!["1", "2", "3", "4"]);

    // All ok.
    for r in &results {
        assert!(r.outcome.is_ok(), "{} failed: {:?}", r.call_id, r.outcome);
    }

    // Reads overlapped: 3x100ms = 300ms serial, but concurrent should
    // finish around 100ms. Write adds another 50ms. Total budget 200ms
    // is generous enough to skip flake on a slow CI box but tight enough
    // to fail if reads accidentally serialize.
    assert!(
        wallclock < Duration::from_millis(200),
        "expected concurrent reads, took {wallclock:?}"
    );

    // Write started after all 3 safe-set tools finished.
    assert_eq!(
        write_seen_after.load(Ordering::SeqCst),
        3,
        "write started before safe set drained"
    );
}

/// Trivial tool used by hook-wired tests below — returns its input tag.
struct EchoTool;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct EchoIn {
    tag: String,
}

#[async_trait]
impl Tool for EchoTool {
    type Input = EchoIn;
    type Output = Tagged;
    fn name(&self) -> &'static str {
        "echo"
    }
    fn description(&self) -> &'static str {
        "echo for hook tests"
    }
    fn parallel_safe(&self) -> bool {
        true
    }
    fn permission(&self, _: &Self::Input) -> PermissionRequest {
        PermissionRequest {
            permission: "read:echo".into(),
            reason: None,
            timeout: None,
        }
    }
    async fn run(&self, _: ToolCtx, i: Self::Input) -> Result<Self::Output, ToolError> {
        Ok(Tagged {
            tag: i.tag,
            finished_at_ms: 0,
        })
    }
}

#[tokio::test]
async fn before_tool_call_replace_mutates_invocation() {
    let registry = ToolRegistry::builder().register(EchoTool).build();
    let permission: Arc<dyn PermissionManager> = Arc::new(AllowAll);
    let dir = TempDir::new().unwrap();

    let mut chains = HookChains::new();
    chains
        .before_tool_call
        .push(HookEntry::<BeforeToolCallCtx> {
            manifest_id: "rewriter".into(),
            priority: Priority(50),
            registration_index: 0,
            kind: HookKind::BeforeToolCall,
            func: Arc::new(|mut c: BeforeToolCallCtx| {
                Box::pin(async move {
                    if let Some(inv) = c.invocation.as_mut() {
                        inv.args = json!({ "tag": "rewritten" });
                    }
                    HookResult::Replace(c)
                })
            }),
        });

    let perm_ctx = PermissionCtx {
        session_id: SessionId::new(),
        mode: PermissionMode::Danger,
    };
    let workspace = dir.path().to_path_buf();
    let ctx_for = move |_inv: &ToolInvocation| ctx(&workspace);

    let invocations = vec![ToolInvocation {
        call_id: "1".into(),
        name: "echo".into(),
        args: json!({ "tag": "original" }),
    }];

    let chains_arc = Arc::new(chains);
    let events: Arc<dyn EventSink> = Arc::new(NoopBus);
    let results = dispatch_batch(
        &registry,
        &permission,
        &chains_arc,
        &events,
        perm_ctx.session_id,
        ctx_for,
        perm_ctx,
        invocations,
    )
    .await;

    let value = results[0].outcome.as_ref().expect("ok");
    let tag = value.get("tag").and_then(|v| v.as_str()).unwrap_or("");
    assert_eq!(tag, "rewritten");
}

#[tokio::test]
async fn before_tool_call_deny_short_circuits_with_permission_denied() {
    let registry = ToolRegistry::builder().register(EchoTool).build();
    let permission: Arc<dyn PermissionManager> = Arc::new(AllowAll);
    let dir = TempDir::new().unwrap();

    let mut chains = HookChains::new();
    chains
        .before_tool_call
        .push(HookEntry::<BeforeToolCallCtx> {
            manifest_id: "blocker".into(),
            priority: Priority(50),
            registration_index: 0,
            kind: HookKind::BeforeToolCall,
            func: Arc::new(|_c: BeforeToolCallCtx| {
                Box::pin(async move {
                    HookResult::Deny {
                        reason: "policy".into(),
                        feedback: Some("blocked by plugin".into()),
                    }
                })
            }),
        });

    let perm_ctx = PermissionCtx {
        session_id: SessionId::new(),
        mode: PermissionMode::Danger,
    };
    let workspace = dir.path().to_path_buf();
    let ctx_for = move |_inv: &ToolInvocation| ctx(&workspace);

    let invocations = vec![ToolInvocation {
        call_id: "1".into(),
        name: "echo".into(),
        args: json!({ "tag": "x" }),
    }];

    let chains_arc = Arc::new(chains);
    let events: Arc<dyn EventSink> = Arc::new(NoopBus);
    let results = dispatch_batch(
        &registry,
        &permission,
        &chains_arc,
        &events,
        perm_ctx.session_id,
        ctx_for,
        perm_ctx,
        invocations,
    )
    .await;

    match &results[0].outcome {
        Err(ToolError::PermissionDenied(msg)) => {
            assert!(msg.contains("blocked by plugin"), "msg: {msg}");
        }
        other => panic!("expected PermissionDenied from before-tool Deny, got {other:?}"),
    }
}

#[tokio::test]
async fn after_tool_call_replace_swaps_result() {
    let registry = ToolRegistry::builder().register(EchoTool).build();
    let permission: Arc<dyn PermissionManager> = Arc::new(AllowAll);
    let dir = TempDir::new().unwrap();

    let mut chains = HookChains::new();
    chains.after_tool_call.push(HookEntry::<AfterToolCallCtx> {
        manifest_id: "redactor".into(),
        priority: Priority(50),
        registration_index: 0,
        kind: HookKind::AfterToolCall,
        func: Arc::new(|mut c: AfterToolCallCtx| {
            Box::pin(async move {
                let (call_id, name) = c
                    .result
                    .as_ref()
                    .map(|r| (r.call_id.clone(), r.name.clone()))
                    .unwrap_or_default();
                c.result = Some(ToolDispatchResult {
                    call_id,
                    name,
                    outcome: Ok(json!({ "tag": "redacted", "finished_at_ms": 0 })),
                });
                HookResult::Replace(c)
            })
        }),
    });

    let perm_ctx = PermissionCtx {
        session_id: SessionId::new(),
        mode: PermissionMode::Danger,
    };
    let workspace = dir.path().to_path_buf();
    let ctx_for = move |_inv: &ToolInvocation| ctx(&workspace);

    let invocations = vec![ToolInvocation {
        call_id: "1".into(),
        name: "echo".into(),
        args: json!({ "tag": "secret" }),
    }];

    let chains_arc = Arc::new(chains);
    let events: Arc<dyn EventSink> = Arc::new(NoopBus);
    let results = dispatch_batch(
        &registry,
        &permission,
        &chains_arc,
        &events,
        perm_ctx.session_id,
        ctx_for,
        perm_ctx,
        invocations,
    )
    .await;

    let value = results[0].outcome.as_ref().expect("ok");
    let tag = value.get("tag").and_then(|v| v.as_str()).unwrap_or("");
    assert_eq!(tag, "redacted");
}

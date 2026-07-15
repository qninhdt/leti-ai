//! Dispatcher tests — parallel-safe partition + permission decisions.
//!
//! Builds a synthetic registry where a `slow_read` tool blocks for a
//! known duration before returning. With `parallel_safe = true`, three
//! `slow_read`s should overlap; the wallclock is bounded well under
//! `3 * single_call_duration`. A `slow_write` (parallel_safe = false)
//! interleaved in the batch must run after the safe set.

mod common;

use common::mock_event_sink::RecordingEventSink;
use common::mock_permission::DenyAll;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bytes::Bytes;
use openlet_adapters::config_perm::ConfigPermissionMgr;
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
use openlet_core::permission::{Deferred, DeferredSender, deferred_pair};
use openlet_core::tools::{
    PromptPolicy, ReadHistory, Tool, ToolDispatchResult, ToolInvocation, ToolRegistry,
    dispatch_batch,
};
use openlet_core::types::agent::AgentId;
use openlet_core::types::event::{AgentEvent, EventFilter};
use openlet_core::types::message::MessageId;
use openlet_core::types::permission::{
    AlwaysScope, AskId, Decision, PermissionAction, PermissionCtx, PermissionMode,
    PermissionRequest, PermissionRule,
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
    fn prompt_policy(&self) -> PromptPolicy {
        PromptPolicy::ContinueOnAsk
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
async fn unsafe_write_is_an_ordered_wave_barrier() {
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

    // Ordered waves preserve the assistant's invocation order: read_a,
    // then write_x, then read_b/read_c concurrently. That is roughly
    // 100 + 50 + 100ms, not the old all-reads-first partitioning.
    assert!(
        wallclock < Duration::from_millis(350),
        "expected the final safe wave to overlap, took {wallclock:?}"
    );

    // The write may start only after the preceding safe wave, not before it
    // or after later safe calls that appear after the barrier.
    assert_eq!(
        write_seen_after.load(Ordering::SeqCst),
        1,
        "write did not respect the preceding safe wave"
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

/// Uses the reserved built-in name to verify the dispatcher-level no-prompt
/// contract without coupling this test to the subagent runtime driver.
struct SubagentNamedTool;

#[async_trait]
impl Tool for SubagentNamedTool {
    type Input = EchoIn;
    type Output = Tagged;

    fn name(&self) -> &'static str {
        "subagent_task"
    }
    fn description(&self) -> &'static str {
        "subagent permission bypass test"
    }
    fn parallel_safe(&self) -> bool {
        false
    }
    fn prompt_policy(&self) -> PromptPolicy {
        PromptPolicy::ContinueOnAsk
    }
    fn permission(&self, _: &Self::Input) -> PermissionRequest {
        PermissionRequest::simple("subagent_task:general")
    }
    async fn run(&self, _: ToolCtx, input: Self::Input) -> Result<Self::Output, ToolError> {
        Ok(Tagged {
            tag: input.tag,
            finished_at_ms: 0,
        })
    }
}

struct ReplyWinsCancel {
    ask_id: AskId,
    deferred: Mutex<Option<Deferred<Decision>>>,
    sender: Mutex<Option<DeferredSender<Decision>>>,
}

impl ReplyWinsCancel {
    fn new() -> Self {
        let (deferred, sender) = deferred_pair(Decision::Deny {
            feedback: Some("orphaned".into()),
        });
        Self {
            ask_id: AskId::new(),
            deferred: Mutex::new(Some(deferred)),
            sender: Mutex::new(Some(sender)),
        }
    }
}

#[async_trait]
impl PermissionManager for ReplyWinsCancel {
    async fn check(
        &self,
        _: PermissionCtx,
        _: PermissionRequest,
    ) -> Result<Decision, PermissionError> {
        Ok(Decision::Pending {
            ask_id: self.ask_id,
        })
    }

    async fn reply(&self, _: AskId, _: Decision) -> Result<(), PermissionError> {
        Ok(())
    }

    async fn cancel_ask(&self, _: AskId) -> Result<(), PermissionError> {
        self.sender
            .lock()
            .unwrap()
            .take()
            .expect("single cancellation attempt")
            .send(Decision::Allow)
            .expect("deferred receiver remains alive");
        Err(PermissionError::AskNotFound)
    }

    async fn record_always(
        &self,
        _: AlwaysScope,
        _: PermissionRule,
    ) -> Result<(), PermissionError> {
        Ok(())
    }

    fn take_deferred(&self, _: AskId) -> Option<Deferred<Decision>> {
        self.deferred.lock().unwrap().take()
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

#[tokio::test]
async fn no_prompt_tool_cancels_pending_ask_and_runs_without_event() {
    let registry = ToolRegistry::builder().register(SubagentNamedTool).build();
    let manager = Arc::new(ConfigPermissionMgr::new());
    let permission: Arc<dyn PermissionManager> = manager.clone();
    let recording = Arc::new(RecordingEventSink::new());
    let events: Arc<dyn EventSink> = recording.clone();
    let dir = TempDir::new().unwrap();
    let workspace = dir.path().to_path_buf();
    let perm_ctx = PermissionCtx {
        session_id: SessionId::new(),
        mode: PermissionMode::ReadOnly,
    };
    let session_id = perm_ctx.session_id;

    let results = dispatch_batch(
        &registry,
        &permission,
        &Arc::new(HookChains::new()),
        &events,
        session_id,
        move |_inv| ctx(&workspace),
        perm_ctx,
        vec![ToolInvocation {
            call_id: "subagent-1".into(),
            name: "subagent_task".into(),
            args: json!({ "tag": "spawned" }),
        }],
    )
    .await;

    let value = results[0].outcome.as_ref().expect("subagent tool runs");
    assert_eq!(value.get("tag").and_then(|v| v.as_str()), Some("spawned"));
    assert_eq!(manager.pending_count(), 0, "pending ask must be cleaned up");
    assert!(
        !recording
            .snapshot()
            .iter()
            .any(|(event, _)| matches!(event, AgentEvent::PermissionAsked { .. })),
        "no-prompt tools must not publish PermissionAsked"
    );
}

#[tokio::test]
async fn no_prompt_tool_preserves_explicit_deny() {
    let registry = ToolRegistry::builder().register(SubagentNamedTool).build();
    let permission: Arc<dyn PermissionManager> = Arc::new(DenyAll);
    let dir = TempDir::new().unwrap();
    let workspace = dir.path().to_path_buf();
    let perm_ctx = PermissionCtx {
        session_id: SessionId::new(),
        mode: PermissionMode::ReadOnly,
    };

    let results = dispatch_batch(
        &registry,
        &permission,
        &Arc::new(HookChains::new()),
        &(Arc::new(NoopBus) as Arc<dyn EventSink>),
        perm_ctx.session_id,
        move |_inv| ctx(&workspace),
        perm_ctx,
        vec![ToolInvocation {
            call_id: "subagent-denied".into(),
            name: "subagent_task".into(),
            args: json!({ "tag": "blocked" }),
        }],
    )
    .await;

    assert!(
        matches!(results[0].outcome, Err(ToolError::PermissionDenied(_))),
        "explicit deny must remain terminal: {:?}",
        results[0].outcome
    );
}

#[tokio::test]
async fn hook_cannot_rename_another_tool_into_subagent_permission_bypass() {
    let registry = ToolRegistry::builder()
        .register(EchoTool)
        .register(SubagentNamedTool)
        .build();
    let permission: Arc<dyn PermissionManager> = Arc::new(DenyAll);
    let dir = TempDir::new().unwrap();
    let workspace = dir.path().to_path_buf();
    let perm_ctx = PermissionCtx {
        session_id: SessionId::new(),
        mode: PermissionMode::ReadOnly,
    };

    let mut chains = HookChains::new();
    chains
        .before_tool_call
        .push(HookEntry::<BeforeToolCallCtx> {
            manifest_id: "renamer".into(),
            priority: Priority(50),
            registration_index: 0,
            kind: HookKind::BeforeToolCall,
            func: Arc::new(|mut c: BeforeToolCallCtx| {
                Box::pin(async move {
                    if let Some(inv) = c.invocation.as_mut() {
                        inv.name = "subagent_task".into();
                    }
                    HookResult::Replace(c)
                })
            }),
        });

    let results = dispatch_batch(
        &registry,
        &permission,
        &Arc::new(chains),
        &(Arc::new(NoopBus) as Arc<dyn EventSink>),
        perm_ctx.session_id,
        move |_inv| ctx(&workspace),
        perm_ctx,
        vec![ToolInvocation {
            call_id: "renamed-1".into(),
            name: "echo".into(),
            args: json!({ "tag": "blocked" }),
        }],
    )
    .await;

    assert!(
        matches!(results[0].outcome, Err(ToolError::PermissionDenied(_))),
        "hook-renamed call must still pass through permission: {:?}",
        results[0].outcome
    );
}

#[tokio::test]
async fn concurrent_reply_wins_over_cancellation_without_conflicting_resolution() {
    let registry = ToolRegistry::builder().register(EchoTool).build();
    let manager = Arc::new(ReplyWinsCancel::new());
    let permission: Arc<dyn PermissionManager> = manager;
    let recording = Arc::new(RecordingEventSink::new());
    let events: Arc<dyn EventSink> = Arc::clone(&recording) as Arc<dyn EventSink>;
    let ctx_events = Arc::clone(&events);
    let dir = TempDir::new().unwrap();
    let workspace = dir.path().to_path_buf();
    let cancel = CancellationToken::new();
    cancel.cancel();
    let perm_ctx = PermissionCtx {
        session_id: SessionId::new(),
        mode: PermissionMode::WorkspaceWrite,
    };

    let results = dispatch_batch(
        &registry,
        &permission,
        &Arc::new(HookChains::new()),
        &events,
        perm_ctx.session_id,
        move |_inv| {
            let mut tool_ctx = ctx(&workspace);
            tool_ctx.cancel = cancel.clone();
            tool_ctx.events = Arc::clone(&ctx_events);
            tool_ctx
        },
        perm_ctx,
        vec![ToolInvocation {
            call_id: "reply-wins".into(),
            name: "echo".into(),
            args: json!({ "tag": "allowed" }),
        }],
    )
    .await;

    let value = results[0]
        .outcome
        .as_ref()
        .expect("reply decision must win");
    assert_eq!(value.get("tag").and_then(|v| v.as_str()), Some("allowed"));
    assert!(
        !recording
            .snapshot()
            .iter()
            .any(|(event, _)| matches!(event, AgentEvent::PermissionResolved { .. })),
        "dispatcher must not publish a second resolution when reply owns the ask"
    );
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

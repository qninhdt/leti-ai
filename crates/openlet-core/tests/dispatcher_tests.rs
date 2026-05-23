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
use openlet_core::error::{ArtifactError, EventError, PermissionError, ToolError};
use openlet_core::tools::{
    ReadHistory, Tool, ToolInvocation, ToolRegistry, dispatch_batch,
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
    async fn check(&self, _: PermissionCtx, _: PermissionRequest) -> Result<Decision, PermissionError> {
        Ok(Decision::Allow)
    }
    async fn reply(&self, _: AskId, _: Decision) -> Result<(), PermissionError> { Ok(()) }
    async fn cancel_ask(&self, _: AskId) -> Result<(), PermissionError> { Ok(()) }
    async fn record_always(&self, _: AlwaysScope, _: PermissionRule) -> Result<(), PermissionError> {
        Ok(())
    }
}

#[derive(Default)]
struct NoopBus;

#[async_trait]
impl EventSink for NoopBus {
    async fn publish(&self, _: AgentEvent, _: Persistence) -> Result<(), EventError> { Ok(()) }
    fn subscribe(&self, _: EventFilter) -> broadcast::Receiver<openlet_core::adapters::event_sink::DeliveredEvent> {
        let (_, rx) = broadcast::channel(1);
        rx
    }
}

#[derive(Default)]
struct DiscardArtifacts;

#[async_trait]
impl ArtifactStore for DiscardArtifacts {
    async fn put(&self, session: SessionId, key: &str, _: Bytes) -> Result<ArtifactRef, ArtifactError> {
        Ok(ArtifactRef { session_id: session, key: key.to_string(), size: 0, mime: None })
    }
    async fn get(&self, _: &ArtifactRef) -> Result<Bytes, ArtifactError> {
        Err(ArtifactError::NotFound("test".into()))
    }
    async fn list(&self, _: SessionId) -> Result<Vec<ArtifactRef>, ArtifactError> { Ok(vec![]) }
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

    fn name(&self) -> &'static str { self.name }
    fn description(&self) -> &'static str { "slow parallel-safe read for tests" }
    fn parallel_safe(&self) -> bool { true }
    fn permission(&self, _: &Self::Input) -> PermissionRequest {
        PermissionRequest { permission: format!("read:{}", self.name), reason: None, timeout: None }
    }

    async fn run(&self, _: ToolCtx, _: Self::Input) -> Result<Self::Output, ToolError> {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let finished_at_ms = self.started_at.elapsed().as_millis();
        Ok(Tagged { tag: self.name.to_string(), finished_at_ms })
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

    fn name(&self) -> &'static str { self.name }
    fn description(&self) -> &'static str { "non-parallel write for tests" }
    fn parallel_safe(&self) -> bool { false }
    fn permission(&self, _: &Self::Input) -> PermissionRequest {
        PermissionRequest { permission: format!("write:{}", self.name), reason: None, timeout: None }
    }

    async fn run(&self, _: ToolCtx, _: Self::Input) -> Result<Self::Output, ToolError> {
        // Snapshot how many parallel-safe tools had finished by the time
        // this serial tool begins. Test asserts it equals the total safe
        // count, proving the partition runs safe-first then serial.
        self.write_seen_after
            .store(self.safe_finished.load(Ordering::SeqCst), Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(50)).await;
        let finished_at_ms = self.started_at.elapsed().as_millis();
        Ok(Tagged { tag: self.name.to_string(), finished_at_ms })
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
        fn name(&self) -> &'static str { self.inner.name() }
        fn description(&self) -> &'static str { self.inner.description() }
        fn parallel_safe(&self) -> bool { self.inner.parallel_safe() }
        fn permission(&self, i: &Self::Input) -> PermissionRequest { self.inner.permission(i) }
        async fn run(&self, ctx: ToolCtx, i: Self::Input) -> Result<Self::Output, ToolError> {
            let r = self.inner.run(ctx, i).await;
            self.counter.fetch_add(1, Ordering::SeqCst);
            r
        }
    }

    let registry = ToolRegistry::builder()
        .register(CountingRead {
            inner: SlowRead { name: "read_a", started_at: Arc::clone(&started) },
            counter: Arc::clone(&safe_finished),
        })
        .register(CountingRead {
            inner: SlowRead { name: "read_b", started_at: Arc::clone(&started) },
            counter: Arc::clone(&safe_finished),
        })
        .register(CountingRead {
            inner: SlowRead { name: "read_c", started_at: Arc::clone(&started) },
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
        ToolInvocation { call_id: "1".into(), name: "read_a".into(), args: json!({}) },
        ToolInvocation { call_id: "2".into(), name: "write_x".into(), args: json!({}) },
        ToolInvocation { call_id: "3".into(), name: "read_b".into(), args: json!({}) },
        ToolInvocation { call_id: "4".into(), name: "read_c".into(), args: json!({}) },
    ];

    let perm_ctx = PermissionCtx { session_id: SessionId::new(), mode: PermissionMode::Danger };
    let workspace = dir.path().to_path_buf();
    let ctx_for = move |_inv: &ToolInvocation| ctx(&workspace);

    let wallclock_start = Instant::now();
    let results = dispatch_batch(&registry, &permission, ctx_for, perm_ctx, invocations).await;
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

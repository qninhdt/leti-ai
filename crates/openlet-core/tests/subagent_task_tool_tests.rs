//! `SubagentTaskTool::run` branch coverage — sync / background / resume,
//! plus `map_spawn_err` code-prefix surfacing — driven by a stub
//! `SubagentSpawner` so no runtime/AppState wiring is needed.
//!
//! Phase 1 (tests-first): pins the model-facing JSON contract of the tool
//! surface before Phase 3 changes the execution model.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use openlet_core::adapters::tool_executor::ToolCtx;
use openlet_core::runtime::subagent::{SpawnError, TaskId, TaskStatus};
use openlet_core::tools::Tool;
use openlet_core::tools::builtins::subagent_task::{
    SpawnedSubagent, SubagentSpawner, SubagentTaskInput, SubagentTaskTool,
};
use openlet_core::types::session::SessionId;

mod common;
use common::tool_ctx::minimal_tool_ctx;

/// Stub spawner: records spawn calls and replays a scripted completion.
/// `await_completion` returns a fixed `(output, cost, status)` triple.
struct StubSpawner {
    spawn_calls: AtomicUsize,
    spawned_types: Mutex<Vec<String>>,
    await_calls: AtomicUsize,
    completion: (String, Option<String>, TaskStatus),
    /// When set, `spawn` returns this error instead of a fresh task id.
    spawn_err: Option<SpawnError>,
    fixed_id: TaskId,
}

impl StubSpawner {
    fn ok(output: &str) -> Self {
        Self {
            spawn_calls: AtomicUsize::new(0),
            spawned_types: Mutex::new(Vec::new()),
            await_calls: AtomicUsize::new(0),
            completion: (
                output.to_string(),
                Some("0.0100".into()),
                TaskStatus::Finished,
            ),
            spawn_err: None,
            fixed_id: TaskId::new(),
        }
    }
}

#[async_trait]
impl SubagentSpawner for StubSpawner {
    async fn spawn(
        &self,
        _ctx: &ToolCtx,
        subagent_type: &str,
        _objective: &str,
        _scope: Option<&str>,
        _background: bool,
    ) -> Result<SpawnedSubagent, SpawnError> {
        self.spawn_calls.fetch_add(1, Ordering::SeqCst);
        self.spawned_types
            .lock()
            .unwrap()
            .push(subagent_type.to_string());
        match &self.spawn_err {
            Some(SpawnError::SubagentQuotaExceeded { in_flight, max }) => {
                Err(SpawnError::SubagentQuotaExceeded {
                    in_flight: *in_flight,
                    max: *max,
                })
            }
            Some(SpawnError::SubagentTypeNotFound(s)) => {
                Err(SpawnError::SubagentTypeNotFound(s.clone()))
            }
            Some(SpawnError::SubagentDepthExceeded { requested, max }) => {
                Err(SpawnError::SubagentDepthExceeded {
                    requested: *requested,
                    max: *max,
                })
            }
            Some(SpawnError::Internal(s)) => Err(SpawnError::Internal(s.clone())),
            Some(SpawnError::SubagentLifetimeBudgetExceeded { spawned, max }) => {
                Err(SpawnError::SubagentLifetimeBudgetExceeded {
                    spawned: *spawned,
                    max: *max,
                })
            }
            Some(SpawnError::MessageRejected(s)) => Err(SpawnError::MessageRejected(s.clone())),
            None => Ok(SpawnedSubagent {
                task_id: self.fixed_id,
                child_session_id: SessionId::new(),
            }),
        }
    }

    async fn await_completion(
        &self,
        _task_id: TaskId,
    ) -> Result<(String, Option<String>, TaskStatus), SpawnError> {
        self.await_calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.completion.clone())
    }
}

fn input(background: bool, task_id: Option<String>) -> SubagentTaskInput {
    SubagentTaskInput {
        subagent_type: Some("worker".into()),
        objective: "do the thing".into(),
        scope: None,
        background,
        task_id,
        child_session_id: None,
    }
}

#[tokio::test]
async fn sync_spawns_then_awaits_and_returns_output_cost() {
    let spawner = Arc::new(StubSpawner::ok("hello from child"));
    let tool = SubagentTaskTool::new(spawner.clone());
    let ctx = minimal_tool_ctx();

    let out = tool.run(ctx, input(false, None)).await.expect("run ok");

    assert_eq!(out.status, "finished");
    assert_eq!(out.output.as_deref(), Some("hello from child"));
    assert_eq!(out.cost_usd.as_deref(), Some("0.0100"));
    assert_eq!(spawner.spawn_calls.load(Ordering::SeqCst), 1);
    assert_eq!(spawner.await_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn omitted_subagent_type_uses_general() {
    let spawner = Arc::new(StubSpawner::ok("general result"));
    let tool = SubagentTaskTool::new(spawner.clone());
    let mut request = input(false, None);
    request.subagent_type = None;

    let out = tool
        .run(minimal_tool_ctx(), request)
        .await
        .expect("general spawn");

    assert_eq!(out.status, "finished");
    assert_eq!(
        spawner.spawned_types.lock().unwrap().as_slice(),
        ["general"]
    );
}

#[tokio::test]
async fn explicit_subagent_type_is_never_rewritten() {
    let spawner = Arc::new(StubSpawner::ok("explicit result"));
    let tool = SubagentTaskTool::new(spawner.clone());
    let mut request = input(false, None);
    request.subagent_type = Some("scout".into());

    tool.run(minimal_tool_ctx(), request)
        .await
        .expect("stub accepts the explicit type");

    assert_eq!(spawner.spawned_types.lock().unwrap().as_slice(), ["scout"]);
}

#[tokio::test]
async fn background_returns_running_without_awaiting() {
    let spawner = Arc::new(StubSpawner::ok("unused"));
    let tool = SubagentTaskTool::new(spawner.clone());
    let ctx = minimal_tool_ctx();

    let out = tool.run(ctx, input(true, None)).await.expect("run ok");

    assert_eq!(out.status, "running");
    assert!(out.output.is_none());
    assert!(out.cost_usd.is_none());
    assert_eq!(spawner.spawn_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        spawner.await_calls.load(Ordering::SeqCst),
        0,
        "background must NOT await"
    );
}

#[tokio::test]
async fn resume_valid_id_sync_skips_spawn_and_awaits() {
    let spawner = Arc::new(StubSpawner::ok("resumed result"));
    let tool = SubagentTaskTool::new(spawner.clone());
    let ctx = minimal_tool_ctx();

    let existing = TaskId::new().0.to_string();
    let out = tool
        .run(ctx, input(false, Some(existing.clone())))
        .await
        .expect("run ok");

    assert_eq!(out.task_id, existing);
    assert_eq!(out.status, "finished");
    assert_eq!(out.output.as_deref(), Some("resumed result"));
    assert_eq!(
        spawner.spawn_calls.load(Ordering::SeqCst),
        0,
        "resume must skip spawn"
    );
    assert_eq!(spawner.await_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn resume_valid_id_background_returns_running_without_await() {
    let spawner = Arc::new(StubSpawner::ok("unused"));
    let tool = SubagentTaskTool::new(spawner.clone());
    let ctx = minimal_tool_ctx();

    let existing = TaskId::new().0.to_string();
    let out = tool
        .run(ctx, input(true, Some(existing.clone())))
        .await
        .expect("run ok");

    assert_eq!(out.task_id, existing);
    assert_eq!(out.status, "running");
    assert_eq!(spawner.spawn_calls.load(Ordering::SeqCst), 0);
    assert_eq!(spawner.await_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn resume_invalid_uuid_falls_through_to_fresh_spawn() {
    let spawner = Arc::new(StubSpawner::ok("fresh spawn result"));
    let tool = SubagentTaskTool::new(spawner.clone());
    let ctx = minimal_tool_ctx();

    // Not a UUID → the resume branch is skipped, fresh spawn runs.
    let out = tool
        .run(ctx, input(false, Some("not-a-uuid".into())))
        .await
        .expect("run ok");

    assert_eq!(out.status, "finished");
    assert_eq!(out.output.as_deref(), Some("fresh spawn result"));
    assert_eq!(
        spawner.spawn_calls.load(Ordering::SeqCst),
        1,
        "invalid uuid must fall through to spawn"
    );
}

#[tokio::test]
async fn map_spawn_err_surfaces_stable_code_prefix() {
    let mut stub = StubSpawner::ok("unused");
    stub.spawn_err = Some(SpawnError::SubagentQuotaExceeded {
        in_flight: 32,
        max: 32,
    });
    let tool = SubagentTaskTool::new(Arc::new(stub));
    let ctx = minimal_tool_ctx();

    let err = tool
        .run(ctx, input(false, None))
        .await
        .expect_err("spawn error must surface");

    // `map_spawn_err` prefixes the stable wire code.
    assert!(
        err.to_string().contains("subagent_quota_exceeded"),
        "error must carry the stable code prefix, got: {err}"
    );
}

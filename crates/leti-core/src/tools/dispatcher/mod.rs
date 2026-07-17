//! Tool dispatcher — the bridge between the typed `ToolRegistry` and
//! the multi-step turn loop.
//!
//! Responsibilities:
//! 1. Permission check: resolves `Decision::Pending` by awaiting the
//!    deferred and treating `Deny` as a tool-error.
//! 2. Ordered waves: each contiguous run of parallel-safe calls overlaps;
//!    every unsafe call is a barrier. This preserves assistant invocation
//!    order around mutations while still allowing sibling reads/tasks to fan
//!    out.
//! 3. Error mapping: tool errors become `ToolResult` parts with `ok:
//!    false` and a model-readable message.

use std::sync::Arc;

use futures::stream::{FuturesUnordered, StreamExt};
use serde_json::Value;

use crate::adapters::event_sink::EventSink;
use crate::adapters::tool_executor::ToolCtx;
use crate::dispatch::{DispatchOutcome, HookChains, dispatch, publish_fault_if_any};
use crate::error::ToolError;
use crate::hooks::io::{AfterToolCallCtx, BeforeToolCallCtx};
use crate::tools::{
    ResourceAccess, ResourceClaim, ResourceKey, SchedulingMode, ToolConcurrency, ToolRegistry,
    ToolScheduler, ToolSchedulerConfig,
};
use crate::types::permission::PermissionCtx;
use crate::types::session::SessionId;

use crate::adapters::permission_manager::PermissionManager;

mod execute;
use execute::run_one;

/// One tool invocation requested by the model.
#[derive(Debug, Clone)]
pub struct ToolInvocation {
    pub call_id: String,
    pub name: String,
    pub args: Value,
}

/// Resolved outcome the runtime hands back to the projection.
#[derive(Debug)]
pub struct ToolDispatchResult {
    pub call_id: String,
    pub name: String,
    pub outcome: Result<Value, ToolError>,
}

/// Emit the `leti_tool_executions_total` counter for one finished
/// tool call. Collapses the byte-identical metric block the parallel
/// and serial arms of [`dispatch_batch`] both used.
fn record_tool_metric(result: &Result<Value, ToolError>, name: &str) {
    metrics::counter!(
        "leti_tool_executions_total",
        "tool" => name.to_string(),
        "outcome" => if result.is_ok() { "ok" } else { "error" },
    )
    .increment(1);
}

/// Dispatch a batch of tool calls. Permission is checked per call; safe
/// tools run concurrently; non-safe tools serialize. Order preserved.
///
/// Plugin hook chains: `before_tool_call` runs before permission check
/// (Deny short-circuits to a synthetic `PermissionDenied`-style result;
/// Replace mutates the invocation's args). `after_tool_call` runs after
/// the tool produces output (Replace swaps the result).
#[allow(clippy::too_many_arguments)]
pub async fn dispatch_batch(
    registry: &Arc<ToolRegistry>,
    permission: &Arc<dyn PermissionManager>,
    hook_chains: &Arc<HookChains>,
    events: &Arc<dyn EventSink>,
    session_id: SessionId,
    ctx_for: impl Fn(&ToolInvocation) -> ToolCtx,
    perm_ctx: PermissionCtx,
    invocations: Vec<ToolInvocation>,
) -> Vec<ToolDispatchResult> {
    dispatch_batch_with_scheduler(
        registry,
        permission,
        hook_chains,
        events,
        session_id,
        ctx_for,
        perm_ctx,
        invocations,
        Arc::new(ToolScheduler::new(ToolSchedulerConfig::default())),
    )
    .await
}

/// Scheduler-backed variant used by the server runtime. The supplied
/// scheduler is process-wide, while this invocation creates one per-turn cap.
#[allow(clippy::too_many_arguments)]
pub async fn dispatch_batch_with_scheduler(
    registry: &Arc<ToolRegistry>,
    permission: &Arc<dyn PermissionManager>,
    hook_chains: &Arc<HookChains>,
    events: &Arc<dyn EventSink>,
    session_id: SessionId,
    ctx_for: impl Fn(&ToolInvocation) -> ToolCtx,
    perm_ctx: PermissionCtx,
    invocations: Vec<ToolInvocation>,
    scheduler: Arc<ToolScheduler>,
) -> Vec<ToolDispatchResult> {
    let len = invocations.len();
    let mut indexed: Vec<(usize, ToolInvocation, ToolConcurrency, Option<ToolError>)> =
        Vec::with_capacity(len);
    // Hooks must precede parsing/classification so an argument replacement
    // locks the resource it actually requests.
    for (i, inv) in invocations.into_iter().enumerate() {
        match before_hooks(hook_chains, events, session_id, &inv).await {
            Ok(inv) => {
                let policy = registry
                    .get(&inv.name)
                    .ok_or_else(|| ToolError::NotFound(inv.name.clone()))
                    .and_then(|t| {
                        Ok(builtin_concurrency(&inv, session_id)
                            .unwrap_or(t.concurrency(&inv.args)?))
                    });
                match policy {
                    Ok(policy) => indexed.push((i, inv, policy, None)),
                    Err(e) => indexed.push((i, inv, ToolConcurrency::exclusive(), Some(e))),
                }
            }
            Err(e) => indexed.push((i, inv, ToolConcurrency::exclusive(), Some(e))),
        }
    }

    let mut out: Vec<Option<ToolDispatchResult>> = (0..len).map(|_| None).collect();

    let turn = scheduler.turn_semaphore();
    let mut cursor = 0;
    while cursor < indexed.len() {
        if indexed[cursor].2.mode == SchedulingMode::Concurrent && indexed[cursor].3.is_none() {
            let wave_start = cursor;
            let mut claims = indexed[cursor].2.claims.clone();
            cursor += 1;
            while cursor < indexed.len()
                && indexed[cursor].2.mode == SchedulingMode::Concurrent
                && indexed[cursor].3.is_none()
                && !claims_conflict(&claims, &indexed[cursor].2.claims)
            {
                claims.extend(indexed[cursor].2.claims.clone());
                cursor += 1;
            }
            let mut futs = FuturesUnordered::new();
            for (idx, inv, policy, _) in indexed[wave_start..cursor].iter().cloned() {
                let registry = Arc::clone(registry);
                let permission = Arc::clone(permission);
                let hooks = Arc::clone(hook_chains);
                let events = Arc::clone(events);
                let ctx = ctx_for(&inv);
                let pctx = perm_ctx.clone();
                let scheduler = scheduler.clone();
                let turn = turn.clone();
                futs.push(async move {
                    let result = run_prepared(
                        &registry,
                        &permission,
                        &hooks,
                        &events,
                        session_id,
                        ctx,
                        pctx,
                        &inv,
                        policy,
                        scheduler,
                        turn,
                    )
                    .await;
                    (idx, inv, result)
                });
            }
            while let Some((idx, inv, result)) = futs.next().await {
                record_tool_metric(&result, &inv.name);
                out[idx] = Some(ToolDispatchResult {
                    call_id: inv.call_id,
                    name: inv.name,
                    outcome: result,
                });
            }
            continue;
        }

        let (idx, inv, policy, pre_error) = indexed[cursor].clone();
        cursor += 1;
        let ctx = ctx_for(&inv);
        let result = match pre_error {
            Some(e) => Err(e),
            None => {
                run_prepared(
                    registry,
                    permission,
                    hook_chains,
                    events,
                    session_id,
                    ctx,
                    perm_ctx.clone(),
                    &inv,
                    policy,
                    scheduler.clone(),
                    turn.clone(),
                )
                .await
            }
        };
        record_tool_metric(&result, &inv.name);
        out[idx] = Some(ToolDispatchResult {
            call_id: inv.call_id,
            name: inv.name,
            outcome: result,
        });
    }

    out.into_iter()
        .enumerate()
        .map(|(i, o)| {
            o.unwrap_or_else(|| ToolDispatchResult {
                call_id: format!("missing-slot-{i}"),
                name: String::new(),
                outcome: Err(ToolError::Io("tool result lost".into())),
            })
        })
        .collect()
}

fn claims_conflict(a: &[ResourceClaim], b: &[ResourceClaim]) -> bool {
    a.iter().any(|x| {
        b.iter().any(|y| {
            x.key == y.key
                && (x.access == ResourceAccess::Write || y.access == ResourceAccess::Write)
        })
    })
}

fn builtin_concurrency(inv: &ToolInvocation, session: SessionId) -> Option<ToolConcurrency> {
    let workspace_read =
        || ToolConcurrency::concurrent().with_claim(ResourceKey::Workspace, ResourceAccess::Read);
    let workspace_write =
        || ToolConcurrency::exclusive().with_claim(ResourceKey::Workspace, ResourceAccess::Write);
    let path = |access| {
        inv.args
            .get("path")
            .and_then(Value::as_str)
            .map(|p| workspace_read().with_claim(ResourceKey::WorkspacePath(p.into()), access))
    };
    match inv.name.as_str() {
        "read" => path(ResourceAccess::Read),
        "write" | "edit" => path(ResourceAccess::Write),
        "list" | "glob" | "grep" => Some(workspace_read()),
        "bash" | "python" => Some(workspace_write()),
        "todo" => Some(ToolConcurrency::concurrent().with_claim(
            ResourceKey::Session(format!("{session}:todos")),
            ResourceAccess::Write,
        )),
        "ask_user" => Some(ToolConcurrency::exclusive().with_claim(
            ResourceKey::Session(format!("{session}:interaction")),
            ResourceAccess::Write,
        )),
        "enter_plan_mode" | "exit_plan_mode" => Some(ToolConcurrency::exclusive().with_claim(
            ResourceKey::Session(format!("{session}:agent-profile")),
            ResourceAccess::Write,
        )),
        "web_fetch" | "subagent_task" | "send_message" => Some(ToolConcurrency::concurrent()),
        "task_status" => inv.args.get("task_id").and_then(Value::as_str).map(|id| {
            ToolConcurrency::concurrent()
                .with_claim(ResourceKey::Task(id.into()), ResourceAccess::Read)
        }),
        _ => None,
    }
}

async fn before_hooks(
    hooks: &Arc<HookChains>,
    events: &Arc<dyn EventSink>,
    session_id: SessionId,
    inv: &ToolInvocation,
) -> Result<ToolInvocation, ToolError> {
    if hooks.before_tool_call.is_empty() {
        return Ok(inv.clone());
    }
    let outcome = dispatch(
        &hooks.before_tool_call,
        BeforeToolCallCtx {
            session_id: Some(session_id),
            invocation: Some(inv.clone()),
        },
    )
    .await;
    publish_fault_if_any(events, Some(session_id), &outcome).await;
    match outcome {
        DispatchOutcome::Completed(c) | DispatchOutcome::Stopped(c) => {
            Ok(c.invocation.unwrap_or_else(|| inv.clone()))
        }
        DispatchOutcome::Denied {
            reason, feedback, ..
        } => Err(ToolError::PermissionDenied(format!(
            "{reason}{}",
            feedback.map(|f| format!(": {f}")).unwrap_or_default()
        ))),
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_prepared(
    registry: &Arc<ToolRegistry>,
    permission: &Arc<dyn PermissionManager>,
    hooks: &Arc<HookChains>,
    events: &Arc<dyn EventSink>,
    session_id: SessionId,
    ctx: ToolCtx,
    perm_ctx: PermissionCtx,
    inv: &ToolInvocation,
    mut policy: ToolConcurrency,
    scheduler: Arc<ToolScheduler>,
    turn: Arc<tokio::sync::Semaphore>,
) -> Result<Value, ToolError> {
    policy.claims = resolve_claims(policy.claims, &ctx);
    let started = std::time::Instant::now();
    let child = ctx.cancel.child_token();
    let admission = scheduler
        .acquire(turn, policy.claims, &child)
        .await
        .map_err(|_| ToolError::Cancelled)?;
    metrics::histogram!("leti_tool_queue_wait_seconds", "stage" => "admission")
        .record(started.elapsed().as_secs_f64());
    // Permission waits race cancellation inside `run_one`, which also cleans
    // up a pending ask before returning. Tool implementations receive the
    // child token in `ToolCtx` and are expected to observe it while running.
    let outcome = run_one(registry, permission, ctx, perm_ctx, inv).await;
    drop(admission);
    after_hooks(hooks, events, session_id, inv, outcome).await
}

fn resolve_claims(claims: Vec<ResourceClaim>, ctx: &ToolCtx) -> Vec<ResourceClaim> {
    claims
        .into_iter()
        .map(|mut c| {
            match c.key {
                ResourceKey::Workspace => {
                    c.key = ResourceKey::Custom {
                        namespace: "filesystem".into(),
                        key: ctx.fs.scheduling_key(std::path::Path::new(".")),
                    }
                }
                ResourceKey::WorkspacePath(ref p) => {
                    c.key = ResourceKey::Custom {
                        namespace: "filesystem".into(),
                        key: ctx.fs.scheduling_key(p),
                    }
                }
                _ => {}
            };
            c
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
#[tracing::instrument(
    skip_all,
    fields(session_id = %session_id, tool = %inv.name, call_id = %inv.call_id)
)]
async fn after_hooks(
    hook_chains: &Arc<HookChains>,
    events: &Arc<dyn EventSink>,
    session_id: SessionId,
    inv: &ToolInvocation,
    outcome: Result<Value, ToolError>,
) -> Result<Value, ToolError> {
    // AfterToolCall — Replace swaps the result; Stop/Deny preserve the
    // original outcome. Same O(1) empty-chain skip as above.
    if hook_chains.after_tool_call.is_empty() {
        return outcome;
    }
    let result_for_hook = ToolDispatchResult {
        call_id: inv.call_id.clone(),
        name: inv.name.clone(),
        outcome: match &outcome {
            Ok(v) => Ok(v.clone()),
            Err(e) => Err(e.clone()),
        },
    };
    let after_ctx = AfterToolCallCtx {
        session_id: Some(session_id),
        invocation: Some(inv.clone()),
        result: Some(result_for_hook),
    };
    let after_outcome = dispatch(&hook_chains.after_tool_call, after_ctx).await;
    publish_fault_if_any(events, Some(session_id), &after_outcome).await;
    match after_outcome {
        DispatchOutcome::Completed(c) | DispatchOutcome::Stopped(c) => {
            if let Some(replaced) = c.result {
                replaced.outcome
            } else {
                outcome
            }
        }
        DispatchOutcome::Denied { .. } => outcome,
    }
}

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
use crate::tools::ToolRegistry;
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

/// Emit the `openlet_tool_executions_total` counter for one finished
/// tool call. Collapses the byte-identical metric block the parallel
/// and serial arms of [`dispatch_batch`] both used.
fn record_tool_metric(result: &Result<Value, ToolError>, name: &str) {
    metrics::counter!(
        "openlet_tool_executions_total",
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
    let len = invocations.len();
    let mut indexed: Vec<(usize, ToolInvocation, bool)> = Vec::with_capacity(len);
    for (i, inv) in invocations.into_iter().enumerate() {
        let parallel_safe = registry.get(&inv.name).is_some_and(|t| t.parallel_safe());
        indexed.push((i, inv, parallel_safe));
    }

    let mut out: Vec<Option<ToolDispatchResult>> = (0..len).map(|_| None).collect();

    let mut cursor = 0;
    while cursor < indexed.len() {
        if indexed[cursor].2 {
            let wave_start = cursor;
            while cursor < indexed.len() && indexed[cursor].2 {
                cursor += 1;
            }
            let mut futs = FuturesUnordered::new();
            for (idx, inv, _) in indexed[wave_start..cursor].iter().cloned() {
                let registry = Arc::clone(registry);
                let permission = Arc::clone(permission);
                let hooks = Arc::clone(hook_chains);
                let events = Arc::clone(events);
                let ctx = ctx_for(&inv);
                let pctx = perm_ctx.clone();
                futs.push(async move {
                    let result = run_one_with_hooks(
                        &registry,
                        &permission,
                        &hooks,
                        &events,
                        session_id,
                        ctx,
                        pctx,
                        &inv,
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

        let (idx, inv, _) = indexed[cursor].clone();
        cursor += 1;
        let ctx = ctx_for(&inv);
        let result = run_one_with_hooks(
            registry,
            permission,
            hook_chains,
            events,
            session_id,
            ctx,
            perm_ctx.clone(),
            &inv,
        )
        .await;
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

#[allow(clippy::too_many_arguments)]
#[tracing::instrument(
    skip_all,
    fields(session_id = %session_id, tool = %inv.name, call_id = %inv.call_id)
)]
async fn run_one_with_hooks(
    registry: &Arc<ToolRegistry>,
    permission: &Arc<dyn PermissionManager>,
    hook_chains: &Arc<HookChains>,
    events: &Arc<dyn EventSink>,
    session_id: SessionId,
    ctx: ToolCtx,
    perm_ctx: PermissionCtx,
    inv: &ToolInvocation,
) -> Result<Value, ToolError> {
    // BeforeToolCall — Replace mutates args; Deny short-circuits to a
    // synthetic error result fed back to the model. Skip ctx clone
    // entirely when no plugin registered the chain (O(1) empty path).
    let mutated = if hook_chains.before_tool_call.is_empty() {
        inv.clone()
    } else {
        let before_ctx = BeforeToolCallCtx {
            session_id: Some(session_id),
            invocation: Some(inv.clone()),
        };
        let outcome = dispatch(&hook_chains.before_tool_call, before_ctx).await;
        publish_fault_if_any(events, Some(session_id), &outcome).await;
        match outcome {
            DispatchOutcome::Completed(c) | DispatchOutcome::Stopped(c) => {
                c.invocation.unwrap_or_else(|| inv.clone())
            }
            DispatchOutcome::Denied {
                reason, feedback, ..
            } => {
                return Err(ToolError::PermissionDenied(format!(
                    "{reason}{}",
                    feedback.map(|f| format!(": {f}")).unwrap_or_default()
                )));
            }
        }
    };

    let outcome = run_one(registry, permission, ctx, perm_ctx, &mutated).await;

    // AfterToolCall — Replace swaps the result; Stop/Deny preserve the
    // original outcome. Same O(1) empty-chain skip as above.
    if hook_chains.after_tool_call.is_empty() {
        return outcome;
    }
    let result_for_hook = ToolDispatchResult {
        call_id: mutated.call_id.clone(),
        name: mutated.name.clone(),
        outcome: match &outcome {
            Ok(v) => Ok(v.clone()),
            Err(e) => Err(e.clone()),
        },
    };
    let after_ctx = AfterToolCallCtx {
        session_id: Some(session_id),
        invocation: Some(mutated),
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

//! Tool dispatcher — the bridge between the typed `ToolRegistry` and
//! the multi-step turn loop.
//!
//! Responsibilities:
//! 1. Permission check: resolves `Decision::Pending` by awaiting the
//!    deferred and treating `Deny` as a tool-error.
//! 2. Parallel-safe partition: splits the per-turn tool_calls into a
//!    safe set (run concurrently via `FuturesUnordered`) and a serial
//!    set (run sequentially). Order is preserved on the way out so the
//!    LLM-message projection stays deterministic.
//! 3. Error mapping: tool errors become `ToolResult` parts with `ok:
//!    false` and a model-readable message.

use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use futures::FutureExt;
use futures::stream::{FuturesUnordered, StreamExt};
use serde_json::Value;

use crate::adapters::event_sink::{EventSink, Persistence};
use crate::adapters::tool_executor::ToolCtx;
use crate::dispatch::{DispatchOutcome, HookChains, dispatch, publish_fault_if_any};
use crate::error::ToolError;
use crate::hooks::io::{AfterToolCallCtx, BeforeToolCallCtx};
use crate::tools::ToolRegistry;
use crate::types::event::AgentEvent;
use crate::types::permission::{Decision, PermissionCtx, PermissionRequest};
use crate::types::session::SessionId;

use crate::adapters::permission_manager::PermissionManager;

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

    // Run parallel-safe set concurrently.
    let safe: Vec<_> = indexed.iter().filter(|(_, _, s)| *s).cloned().collect();
    let mut futs = FuturesUnordered::new();
    for (idx, inv, _) in safe {
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
        out[idx] = Some(ToolDispatchResult {
            call_id: inv.call_id,
            name: inv.name,
            outcome: result,
        });
    }

    // Run non-safe set serially.
    for (idx, inv, _) in indexed.into_iter().filter(|(_, _, s)| !*s) {
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

async fn run_one(
    registry: &Arc<ToolRegistry>,
    permission: &Arc<dyn PermissionManager>,
    ctx: ToolCtx,
    perm_ctx: PermissionCtx,
    inv: &ToolInvocation,
) -> Result<Value, ToolError> {
    let tool = registry
        .get(&inv.name)
        .ok_or_else(|| ToolError::NotFound(inv.name.clone()))?;
    let req: PermissionRequest = tool.permission(&inv.args)?;
    let req_for_event = req.clone();
    let decision = permission
        .check(perm_ctx, req)
        .await
        .map_err(|e| ToolError::Io(format!("permission check failed: {e}")))?;
    match decision {
        Decision::Allow => {}
        Decision::Deny { feedback } => {
            return Err(ToolError::PermissionDenied(
                feedback.unwrap_or_else(|| "denied by ruleset".into()),
            ));
        }
        Decision::Pending { ask_id } => {
            // A pending decision is invisible to clients until the ask is
            // announced — without this event no frontend can render a
            // prompt, no human can reply, and the `deferred` below never
            // resolves, parking the whole turn loop indefinitely. Publish
            // BEFORE taking/awaiting the deferred so the prompt is on the
            // wire even if the await is cancelled an instant later.
            let _ = ctx
                .events
                .publish(
                    AgentEvent::PermissionAsked {
                        session_id: ctx.session_id,
                        ask_id,
                        request: req_for_event,
                    },
                    Persistence::Durable,
                )
                .await;
            // Take ownership of the deferred half from the manager. The
            // sender is held by the manager (resolved via reply / sweep
            // / accept_ask), and we await the receiver. Drop-resolves to
            // Deny so a dropped sender doesn't hang us.
            let deferred = permission
                .take_deferred(ask_id)
                .ok_or_else(|| ToolError::PermissionDenied("ask expired".into()))?;
            // Race the deferred against ctx.cancel so a session cancel
            // (TUI abort, plugin termination, server shutdown) doesn't
            // leave the tool call parked forever.
            let resolved = tokio::select! {
                d = deferred => d,
                () = ctx.cancel.cancelled() => {
                    return Err(ToolError::PermissionDenied("cancelled".into()));
                }
            };
            match resolved {
                Decision::Allow => {}
                Decision::Deny { feedback } => {
                    return Err(ToolError::PermissionDenied(
                        feedback.unwrap_or_else(|| "denied by user".into()),
                    ));
                }
                Decision::Pending { .. } => {
                    return Err(ToolError::Io(
                        "permission deferred resolved to Pending (unreachable)".into(),
                    ));
                }
            }
        }
    }
    // Catch panics inside `run_json` so a buggy tool can't unwind the
    // turn loop. Mirrors the dispatch hook protection in `dispatch.rs`.
    match AssertUnwindSafe(tool.run_json(ctx, inv.args.clone()))
        .catch_unwind()
        .await
    {
        Ok(res) => res,
        Err(_) => Err(ToolError::Io(format!("tool '{}' panicked", inv.name))),
    }
}

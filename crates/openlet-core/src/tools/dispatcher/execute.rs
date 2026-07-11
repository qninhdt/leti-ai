//! Single-tool execution: permission check (incl. `Pending` deferred
//! await) + panic-catching `run_json`.
//!
//! Extracted verbatim from `dispatcher.rs` so the module keeps the
//! batch/partition + hook-chain scaffolding while this file owns the
//! intricate per-call permission + execution path.

use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use futures::FutureExt;
use serde_json::Value;

use crate::adapters::event_sink::Persistence;
use crate::adapters::permission_manager::PermissionManager;
use crate::adapters::tool_executor::ToolCtx;
use crate::error::ToolError;
use crate::tools::ToolRegistry;
use crate::types::event::AgentEvent;
use crate::types::permission::{Decision, PermissionCtx, PermissionRequest};

use super::ToolInvocation;

pub(super) async fn run_one(
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

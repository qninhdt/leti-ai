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

use std::sync::Arc;

use futures::stream::{FuturesUnordered, StreamExt};
use serde_json::Value;

use crate::adapters::tool_executor::ToolCtx;
use crate::error::ToolError;
use crate::tools::ToolRegistry;
use crate::types::permission::{Decision, PermissionCtx, PermissionRequest};

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
pub async fn dispatch_batch(
    registry: &Arc<ToolRegistry>,
    permission: &Arc<dyn PermissionManager>,
    ctx_for: impl Fn(&ToolInvocation) -> ToolCtx,
    perm_ctx: PermissionCtx,
    invocations: Vec<ToolInvocation>,
) -> Vec<ToolDispatchResult> {
    let len = invocations.len();
    let mut indexed: Vec<(usize, ToolInvocation, bool)> = Vec::with_capacity(len);
    for (i, inv) in invocations.into_iter().enumerate() {
        let parallel_safe = registry
            .get(&inv.name)
            .is_some_and(|t| t.parallel_safe());
        indexed.push((i, inv, parallel_safe));
    }

    let mut out: Vec<Option<ToolDispatchResult>> = (0..len).map(|_| None).collect();

    // Run parallel-safe set concurrently.
    let safe: Vec<_> = indexed.iter().filter(|(_, _, s)| *s).cloned().collect();
    let mut futs = FuturesUnordered::new();
    for (idx, inv, _) in safe {
        let registry = Arc::clone(registry);
        let permission = Arc::clone(permission);
        let ctx = ctx_for(&inv);
        let pctx = perm_ctx.clone();
        futs.push(async move {
            let result = run_one(&registry, &permission, ctx, pctx, &inv).await;
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
        let result = run_one(registry, permission, ctx, perm_ctx.clone(), &inv).await;
        out[idx] = Some(ToolDispatchResult {
            call_id: inv.call_id,
            name: inv.name,
            outcome: result,
        });
    }

    out.into_iter()
        .map(|o| o.expect("every slot filled"))
        .collect()
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
        Decision::Pending { ask_id: _ } => {
            // Phase 4C plumbs the runtime to `take_deferred(ask_id)` on a
            // `ConfigPermissionMgr`-backed manager. The trait surface
            // returns `Pending` and lets the runtime drive the wait;
            // that wiring lands in phase 5 alongside the SSE permission
            // event. For now treat as deny so dispatch never hangs.
            return Err(ToolError::PermissionDenied(
                "permission ask received but no driver wired (phase 5)".into(),
            ));
        }
    }
    tool.run_json(ctx, inv.args.clone()).await
}

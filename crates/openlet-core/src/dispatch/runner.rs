//! Hook chain runner — the panic/timeout-isolated execution loop that
//! drives a sorted `&[HookEntry<I>]` to a `DispatchOutcome<I>`.
//!
//! Extracted from `dispatch.rs` so the runner machinery (HOOK_TIMEOUT,
//! `dispatch`, `dispatch_event`, `downcast_panic`) lives separately from
//! the chain types + helpers (HookEntry, HookChains, PluginFault,
//! `publish_*`).

use std::any::Any;
use std::panic::AssertUnwindSafe;
use std::time::Duration;

use futures::FutureExt;

use super::{
    DispatchOutcome, FaultKind, HookEntry, HookResult, OnEventCtx, PluginFault,
};

/// Per-hook execution timeout. A hook future that doesn't complete
/// within this surfaces as `Denied` (panic/timeout discriminator).
pub(crate) const HOOK_TIMEOUT: Duration = Duration::from_secs(5);

/// Iterate `chain` honoring the four `HookResult` outcomes and the two
/// safety nets (panic-at-construction, panic/timeout-while-polling).
/// Cf. `dispatch.rs::dispatch` doc comment for the full contract.
pub async fn dispatch<I>(chain: &[HookEntry<I>], mut input: I) -> DispatchOutcome<I>
where
    I: Send + 'static,
{
    for entry in chain {
        let func = entry.func.clone();
        let manifest_id = entry.manifest_id.clone();
        let kind = entry.kind;
        let fut = match std::panic::catch_unwind(AssertUnwindSafe(|| func(input))) {
            Ok(f) => f,
            Err(payload) => {
                let panic_msg = downcast_panic(payload);
                tracing::error!(
                    plugin = %manifest_id,
                    hook = ?kind,
                    phase = "construction",
                    panic = %panic_msg,
                    "plugin hook panicked; chain halted",
                );
                return DispatchOutcome::Denied {
                    reason: format!("plugin {manifest_id} hook {kind:?} panicked at construction"),
                    feedback: None,
                    plugin_fault: Some(PluginFault {
                        plugin_id: manifest_id.clone(),
                        hook: kind,
                        kind: FaultKind::ConstructionPanic,
                        message: panic_msg,
                    }),
                };
            }
        };
        let polled = tokio::time::timeout(HOOK_TIMEOUT, AssertUnwindSafe(fut).catch_unwind()).await;
        match polled {
            Err(_) => {
                tracing::error!(
                    plugin = %manifest_id,
                    hook = ?kind,
                    timeout_ms = HOOK_TIMEOUT.as_millis() as u64,
                    "plugin hook exceeded timeout; chain halted",
                );
                return DispatchOutcome::Denied {
                    reason: format!("plugin {manifest_id} hook {kind:?} timed out"),
                    feedback: None,
                    plugin_fault: Some(PluginFault {
                        plugin_id: manifest_id.clone(),
                        hook: kind,
                        kind: FaultKind::Timeout,
                        message: format!("exceeded {}ms hook timeout", HOOK_TIMEOUT.as_millis(),),
                    }),
                };
            }
            Ok(Ok(HookResult::Continue(next))) => input = next,
            Ok(Ok(HookResult::Replace(next))) => {
                tracing::info!(
                    plugin = %manifest_id,
                    hook = ?kind,
                    "plugin hook returned Replace; mutated context forwarded",
                );
                input = next;
            }
            Ok(Ok(HookResult::Stop(next))) => return DispatchOutcome::Stopped(next),
            Ok(Ok(HookResult::Deny { reason, feedback })) => {
                return DispatchOutcome::Denied {
                    reason,
                    feedback,
                    plugin_fault: None,
                };
            }
            Ok(Err(payload)) => {
                let panic_msg = downcast_panic(payload);
                tracing::error!(
                    plugin = %manifest_id,
                    hook = ?kind,
                    phase = "polling",
                    panic = %panic_msg,
                    "plugin hook panicked; chain halted",
                );
                return DispatchOutcome::Denied {
                    reason: format!("plugin {manifest_id} hook {kind:?} panicked while awaiting"),
                    feedback: None,
                    plugin_fault: Some(PluginFault {
                        plugin_id: manifest_id.clone(),
                        hook: kind,
                        kind: FaultKind::PollPanic,
                        message: panic_msg,
                    }),
                };
            }
        }
    }
    DispatchOutcome::Completed(input)
}

/// Specialized runner for `HookKind::OnEvent` — wraps `dispatch` and
/// downgrades `Stopped`/`Denied` outcomes to `Completed` so a buggy
/// observer plugin cannot swallow events for downstream observers.
pub async fn dispatch_event(chain: &[HookEntry<OnEventCtx>], input: OnEventCtx) -> OnEventCtx {
    let original = OnEventCtx {
        event: input.event.clone(),
    };
    match dispatch(chain, input).await {
        DispatchOutcome::Completed(ctx) => ctx,
        DispatchOutcome::Stopped(ctx) => {
            tracing::warn!(
                "on_event hook returned Stop; downgraded to Continue (firehose contract)"
            );
            ctx
        }
        DispatchOutcome::Denied { .. } => {
            tracing::warn!(
                "on_event hook returned Deny / panicked / timed out; original event preserved"
            );
            original
        }
    }
}

pub(super) fn downcast_panic(payload: Box<dyn Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        return (*s).to_string();
    }
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.clone();
    }
    "(non-string panic payload)".to_string()
}

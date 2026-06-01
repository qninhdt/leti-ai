//! Per-dispatch tool-allowlist enforcement.
//!
//! Snapshot of the active agent's `tool_allowlist` resolved RIGHT
//! BEFORE each tool batch (not at turn start) — so a previous tool in
//! the same loop can swap the agent (e.g. `EnterPlanMode`) and the
//! next batch sees the new gate. Disallowed calls short-circuit with
//! `ToolError::NotAllowedInAgent` so the model receives a corrected
//! error and can pivot to an allowed tool.

use std::sync::Arc;

use crate::adapters::memory_store::MemoryStore;
use crate::agent::{AgentRegistry, AgentSlug};
use crate::error::ToolError;
use crate::tools::{ToolDispatchResult, ToolInvocation};
use crate::types::session::SessionId;

/// Snapshot of the tool allowlist active for the next dispatch.
/// `agent_slug` is the slug we resolved from the session at this exact
/// moment — used in `ToolError::NotAllowedInAgent` so the model sees a
/// stable label.
pub struct AllowlistSnapshot {
    pub agent_slug: String,
    pub allowlist: Vec<String>,
}

/// Resolve the current agent's allowlist by reading the session's
/// `current_agent_slug` and looking it up in the registry. Returns
/// `None` when allowlist enforcement is disabled (no `agent_registry`
/// on `LoopContext`) — legacy callers and tests still work without
/// touching `MemoryStore::switch_agent`.
pub async fn resolve_allowlist(
    memory: &Arc<dyn MemoryStore>,
    session_id: SessionId,
    registry: Option<&Arc<AgentRegistry>>,
) -> Option<AllowlistSnapshot> {
    let registry = registry?;
    let meta = memory.get_session(session_id).await.ok().flatten()?;
    let slug_str = meta.current_agent_slug.as_deref()?;
    let slug = AgentSlug::new(slug_str.to_string()).ok()?;
    let def = registry.get(&slug)?;
    if def.tool_allowlist.is_empty() {
        return None;
    }
    Some(AllowlistSnapshot {
        agent_slug: slug_str.to_string(),
        allowlist: def.tool_allowlist.clone(),
    })
}

/// Split `invocations` into (allowed, denied) using the allowlist
/// snapshot. When `snapshot` is `None`, every call is allowed (legacy
/// path). The denied vector pairs the original invocation with a
/// pre-built `ToolError::NotAllowedInAgent` so the merge step can
/// fold them into the result list at their original positions.
pub fn partition_by_allowlist(
    invocations: &[ToolInvocation],
    snapshot: Option<&AllowlistSnapshot>,
) -> (Vec<ToolInvocation>, Vec<(usize, ToolInvocation, ToolError)>) {
    match snapshot {
        None => (invocations.to_vec(), Vec::new()),
        Some(snap) => {
            let mut allowed = Vec::with_capacity(invocations.len());
            let mut denied = Vec::new();
            for (idx, inv) in invocations.iter().enumerate() {
                if snap.allowlist.iter().any(|n| n == &inv.name) {
                    allowed.push(inv.clone());
                } else {
                    denied.push((
                        idx,
                        inv.clone(),
                        ToolError::NotAllowedInAgent {
                            tool: inv.name.clone(),
                            agent: snap.agent_slug.clone(),
                        },
                    ));
                }
            }
            (allowed, denied)
        }
    }
}

/// Stitch dispatched results + denied placeholders back into the
/// invocation order the model emitted. Preserving order matters for
/// the projection step — the LLM expects each tool_call to map to the
/// matching tool result by position.
pub fn merge_with_denied(
    invocations: &[ToolInvocation],
    dispatched: Vec<ToolDispatchResult>,
    denied: Vec<(usize, ToolInvocation, ToolError)>,
) -> Vec<ToolDispatchResult> {
    let mut by_call_id: std::collections::HashMap<String, ToolDispatchResult> = dispatched
        .into_iter()
        .map(|r| (r.call_id.clone(), r))
        .collect();
    let mut denied_by_idx: std::collections::HashMap<usize, ToolDispatchResult> = denied
        .into_iter()
        .map(|(idx, inv, err)| {
            (
                idx,
                ToolDispatchResult {
                    call_id: inv.call_id,
                    name: inv.name,
                    outcome: Err(err),
                },
            )
        })
        .collect();
    let mut out = Vec::with_capacity(invocations.len());
    for (idx, inv) in invocations.iter().enumerate() {
        if let Some(d) = denied_by_idx.remove(&idx) {
            out.push(d);
        } else if let Some(d) = by_call_id.remove(&inv.call_id) {
            out.push(d);
        } else {
            // Defensive — shouldn't happen because dispatched +
            // denied partition the original list. Surface as IO error
            // so the model sees something rather than a silent gap.
            // M3 — log it: a hit here means a real partition bug (a
            // call_id present in neither `dispatched` nor `denied`), and
            // without a forensic trace the synthetic IO error is the only
            // (silent) signal. No metrics infra in MVP; a tracing line is
            // sufficient for post-incident diagnosis.
            tracing::error!(
                call_id = %inv.call_id,
                tool = %inv.name,
                "dispatch slot lost — partition mismatch in merge_with_denied"
            );
            out.push(ToolDispatchResult {
                call_id: inv.call_id.clone(),
                name: inv.name.clone(),
                outcome: Err(ToolError::Io("dispatch slot lost".into())),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn inv(call_id: &str, name: &str) -> ToolInvocation {
        ToolInvocation {
            call_id: call_id.into(),
            name: name.into(),
            args: json!({}),
        }
    }

    #[test]
    fn no_snapshot_lets_everything_through() {
        let invs = vec![inv("1", "read"), inv("2", "write")];
        let (allowed, denied) = partition_by_allowlist(&invs, None);
        assert_eq!(allowed.len(), 2);
        assert!(denied.is_empty());
    }

    #[test]
    fn allowlist_filters_denied_set() {
        let snap = AllowlistSnapshot {
            agent_slug: "plan".into(),
            allowlist: vec!["read".into(), "list".into()],
        };
        let invs = vec![inv("1", "read"), inv("2", "write"), inv("3", "list")];
        let (allowed, denied) = partition_by_allowlist(&invs, Some(&snap));
        assert_eq!(allowed.len(), 2);
        assert_eq!(denied.len(), 1);
        let (idx, _, err) = &denied[0];
        assert_eq!(*idx, 1);
        match err {
            ToolError::NotAllowedInAgent { tool, agent } => {
                assert_eq!(tool, "write");
                assert_eq!(agent, "plan");
            }
            other => panic!("expected NotAllowedInAgent, got {other:?}"),
        }
    }

    #[test]
    fn merge_preserves_invocation_order() {
        let invs = vec![inv("1", "read"), inv("2", "write"), inv("3", "list")];
        let dispatched = vec![
            ToolDispatchResult {
                call_id: "1".into(),
                name: "read".into(),
                outcome: Ok(json!("ok-1")),
            },
            ToolDispatchResult {
                call_id: "3".into(),
                name: "list".into(),
                outcome: Ok(json!("ok-3")),
            },
        ];
        let denied = vec![(
            1,
            inv("2", "write"),
            ToolError::NotAllowedInAgent {
                tool: "write".into(),
                agent: "plan".into(),
            },
        )];
        let merged = merge_with_denied(&invs, dispatched, denied);
        let ids: Vec<&str> = merged.iter().map(|r| r.call_id.as_str()).collect();
        assert_eq!(ids, vec!["1", "2", "3"]);
        assert!(merged[0].outcome.is_ok());
        assert!(matches!(
            merged[1].outcome,
            Err(ToolError::NotAllowedInAgent { .. })
        ));
        assert!(merged[2].outcome.is_ok());
    }
}

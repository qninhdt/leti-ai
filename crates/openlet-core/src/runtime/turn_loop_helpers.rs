//! Crate-internal helpers for `runtime::turn_loop`.
//!
//! Moved out of `turn_loop.rs` to keep the loop body focused on control
//! flow. These are pure I/O helpers against `MemoryStore` + `EventSink`
//! plus a doom-guard summary builder. All callers live in the parent
//! `turn_loop` module, hence `pub(super)` visibility.

use std::sync::Arc;

use chrono::Utc;

use crate::adapters::event_sink::{EventSink, Persistence};
use crate::adapters::memory_store::MemoryStore;
use crate::error::CoreError;
use crate::projection::LlmMessage;
use crate::runtime::doom_guard::{ToolCallSig, TurnSummary as DoomTurnSummary};
use crate::tools::{ToolDispatchResult, ToolInvocation};
use crate::types::event::AgentEvent;
use crate::types::message::{Message, MessageId, Role};
use crate::types::part::{Part, PartId};
use crate::types::session::SessionId;

/// Collect every `Part::ToolCall` on a single assistant message into
/// the canonical `ToolInvocation` shape consumed by `dispatch_batch`.
pub(super) async fn collect_tool_calls(
    memory: &Arc<dyn MemoryStore>,
    session_id: SessionId,
    message_id: MessageId,
) -> Result<Vec<ToolInvocation>, CoreError> {
    let parts = memory.list_parts(session_id, message_id).await?;
    let mut out = Vec::new();
    for p in parts {
        if let Part::ToolCall {
            call_id,
            name,
            args,
            ..
        } = p
        {
            out.push(ToolInvocation {
                call_id,
                name,
                args,
            });
        }
    }
    Ok(out)
}

/// Concatenate every `Part::Text` body on a single message. Used after a
/// compaction turn to fold the assistant's reply into a Compaction part.
pub(super) async fn collect_assistant_text(
    memory: &Arc<dyn MemoryStore>,
    session_id: SessionId,
    message_id: MessageId,
) -> Result<String, CoreError> {
    let parts = memory.list_parts(session_id, message_id).await?;
    let mut buf = String::new();
    for p in parts {
        if let Part::Text { text, .. } = p {
            if !buf.is_empty() {
                buf.push('\n');
            }
            buf.push_str(&text);
        }
    }
    Ok(buf)
}

/// Append a fresh `Role::Tool` message holding all dispatch results as
/// `Part::ToolResult` parts. Emits `MessageCreated` + `PartCreated`
/// events for every appended part.
pub(super) async fn append_tool_message(
    memory: &Arc<dyn MemoryStore>,
    events: &Arc<dyn EventSink>,
    session_id: SessionId,
    results: &[ToolDispatchResult],
) -> Result<MessageId, CoreError> {
    let msg = Message {
        id: MessageId::new(),
        session_id,
        role: Role::Tool,
        created_at: Utc::now(),
    };
    let mid = memory.append_message(session_id, msg).await?;
    events
        .publish(
            AgentEvent::MessageCreated {
                session_id,
                message_id: mid,
                at: Utc::now(),
            },
            Persistence::Durable,
        )
        .await?;

    for r in results {
        let part_id = PartId::new();
        let part = match &r.outcome {
            Ok(value) => Part::ToolResult {
                id: part_id,
                call_id: r.call_id.clone(),
                ok: true,
                text: Some(value.to_string()),
                error: None,
            },
            Err(err) => Part::ToolResult {
                id: part_id,
                call_id: r.call_id.clone(),
                ok: false,
                text: None,
                error: Some(err.to_string()),
            },
        };
        memory.append_part(mid, part).await?;
        events
            .publish(
                AgentEvent::PartCreated {
                    session_id,
                    message_id: mid,
                    part_id,
                    at: Utc::now(),
                },
                Persistence::Durable,
            )
            .await?;
    }
    Ok(mid)
}

/// Re-project the persisted session into the LLM-message shape consumed
/// by `run_turn`. Always re-fetches messages + parts from `MemoryStore`
/// so the next turn sees Compaction substitutions and any
/// out-of-band writes.
pub(super) async fn project_session_messages(
    memory: &Arc<dyn MemoryStore>,
    session_id: SessionId,
) -> Result<Vec<LlmMessage>, CoreError> {
    use std::collections::HashMap;

    use crate::projection::{ProjectionCaps, project_for_llm};
    let messages = memory.list_messages(session_id).await?;
    let mut parts_by_msg: HashMap<MessageId, Vec<Part>> = HashMap::with_capacity(messages.len());
    for m in &messages {
        let parts = memory.list_parts(session_id, m.id).await?;
        parts_by_msg.insert(m.id, parts);
    }
    Ok(project_for_llm(
        &messages,
        &parts_by_msg,
        ProjectionCaps::default(),
    ))
}

/// Build a `TurnSummary` for the doom-guard from the assistant message just
/// produced + the freshly-dispatched tool results. `had_text_output` reflects
/// any non-empty `Part::Text` body; `had_successful_writes` is any tool result
/// with `ok=true` from a tool the registry marks `parallel_safe=false` (write
/// tools serialize). Tool-call signatures use `ToolCallSig::new` over the
/// invocation's parsed args.
pub(super) async fn build_doom_summary(
    memory: &Arc<dyn MemoryStore>,
    session_id: SessionId,
    assistant_message_id: MessageId,
    invocations: &[ToolInvocation],
    results: &[ToolDispatchResult],
) -> Result<DoomTurnSummary, CoreError> {
    let parts = memory.list_parts(session_id, assistant_message_id).await?;
    let had_text_output = parts
        .iter()
        .any(|p| matches!(p, Part::Text { text, .. } if !text.trim().is_empty()));
    let had_successful_writes = results
        .iter()
        .any(|r| r.outcome.is_ok() && !is_read_only_tool(&r.name));
    let mut tool_calls = std::collections::BTreeSet::new();
    for inv in invocations {
        tool_calls.insert(ToolCallSig::new(inv.name.clone(), &inv.args));
    }
    Ok(DoomTurnSummary {
        had_text_output,
        had_successful_writes,
        tool_calls,
    })
}

/// Names of read-only tools — used by the doom-guard to discriminate
/// "writes succeeded" from "reads succeeded." Read-only successes don't
/// reset the loop counter: an agent looping on `read` of the same path
/// is still looping.
fn is_read_only_tool(name: &str) -> bool {
    matches!(name, "read" | "list" | "glob" | "grep")
}

//! Pure incremental processor — `ChatDelta` → assistant message parts + events.
//!
//! Stateless logic on top of `ProcessorState`. No IO. No async. The runtime
//! drives this from the provider's stream and dispatches the resulting
//! writes/events to storage + the bus.
//!
//! Args are accumulated in
//! a `String` buffer, parsed only on `Finish`. A duplicate `(name, index)`
//! triggers a typed `ProviderError::Decode` mid-stream.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::adapters::model_provider::{ChatDelta, FinishReason};

/// Hard cap on `pending_tool_calls` per turn. An adversarial provider
/// stream could announce `tool_call_index = u32::MAX` repeatedly and
/// grow the BTreeMap without bound. Closes ISSUE-A11.
const MAX_PENDING_TOOL_CALLS: usize = 64;
use crate::error::ProviderError;
use crate::types::event::{DeltaKind, Usage};

/// Accumulator for a single in-progress tool call.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PendingToolCall {
    pub call_id: String,
    pub name: String,
    pub args_buf: String,
}

/// State carried between `Processor::step` calls within one assistant turn.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProcessorState {
    pub current_text: String,
    pub current_reasoning: String,
    pub current_reasoning_signature: Option<String>,
    pub pending_tool_calls: BTreeMap<usize, PendingToolCall>,
    pub usage: Option<Usage>,
    pub finish: Option<FinishReason>,
}

/// Materialized assistant-side part the runtime should persist. The runtime
/// owns id assignment + storage; the processor only emits the payload.
#[derive(Debug, Clone, PartialEq)]
pub enum ProcessorPart {
    Text {
        text: String,
    },
    Reasoning {
        text: String,
        signature: Option<String>,
    },
    ToolCall {
        call_id: String,
        name: String,
        args: serde_json::Value,
    },
    StepFinish {
        reason: String,
        usage: Option<Usage>,
    },
}

/// Event the runtime should publish on the bus. Persistence flag (durable
/// vs transient) is decided at the runtime layer.
#[derive(Debug, Clone, PartialEq)]
pub enum ProcessorEvent {
    /// Streaming delta — transient. `delta` is the new fragment ONLY.
    PartDelta { kind: DeltaKind, delta: String },
}

/// One step's output: parts to persist + events to publish + next state.
#[derive(Debug, Clone)]
pub struct StepOutcome {
    pub parts: Vec<ProcessorPart>,
    pub events: Vec<ProcessorEvent>,
    pub next: ProcessorState,
}

/// Pure processor — `step` is a total function: same `(state, delta)` ⇒
/// same outcome.
#[derive(Debug, Default)]
pub struct Processor;

impl Processor {
    /// Reject a brand-new tool-call index once the pending map is at the
    /// cap. Existing indices are always allowed (they don't grow the map).
    /// Shared by `ToolCallStart` and `ToolCallArgsDelta` so an adversarial
    /// stream can't bypass the bound via either entry point (ISSUE-A11).
    fn reject_if_over_cap(
        pending: &BTreeMap<usize, PendingToolCall>,
        index: usize,
    ) -> Result<(), ProviderError> {
        if pending.len() >= MAX_PENDING_TOOL_CALLS && !pending.contains_key(&index) {
            return Err(ProviderError::Decode(format!(
                "too many pending tool calls (cap {MAX_PENDING_TOOL_CALLS})"
            )));
        }
        Ok(())
    }

    /// Apply one `ChatDelta`. Returns parts/events to persist+publish and
    /// the next state. Errors short-circuit the turn (the runtime should
    /// stop draining the stream and finalize with `reason=error`).
    pub fn step(mut state: ProcessorState, delta: ChatDelta) -> Result<StepOutcome, ProviderError> {
        let mut parts: Vec<ProcessorPart> = Vec::new();
        let mut events: Vec<ProcessorEvent> = Vec::new();

        match delta {
            ChatDelta::Role => {
                // No-op for the storage layer; some providers send Role once.
            }
            ChatDelta::Content { text } => {
                if !text.is_empty() {
                    state.current_text.push_str(&text);
                    events.push(ProcessorEvent::PartDelta {
                        kind: DeltaKind::Text,
                        delta: text,
                    });
                }
            }
            ChatDelta::Reasoning { text, signature } => {
                if signature.is_some() {
                    state.current_reasoning_signature = signature;
                }
                if !text.is_empty() {
                    state.current_reasoning.push_str(&text);
                    events.push(ProcessorEvent::PartDelta {
                        kind: DeltaKind::Reasoning,
                        delta: text,
                    });
                }
            }
            ChatDelta::ToolCallStart {
                call_id,
                name,
                index,
            } => {
                if let Some(existing) = state.pending_tool_calls.get(&index) {
                    let name_collision =
                        !existing.name.is_empty() && !name.is_empty() && existing.name != name;
                    let already_streaming = !existing.args_buf.is_empty();
                    if name_collision || already_streaming {
                        return Err(ProviderError::Decode(format!(
                            "duplicate tool_call index {index} (existing call_id={}, \
                             incoming call_id={call_id})",
                            existing.call_id
                        )));
                    }
                }
                Self::reject_if_over_cap(&state.pending_tool_calls, index)?;
                state.pending_tool_calls.insert(
                    index,
                    PendingToolCall {
                        call_id,
                        name,
                        args_buf: String::new(),
                    },
                );
            }
            ChatDelta::ToolCallArgsDelta { index, args_chunk } => {
                // Same bound as ToolCallStart: an args delta for a brand-new
                // index would otherwise insert via `or_default()` and bypass
                // the cap, letting an adversarial stream grow the map without
                // bound (ISSUE-A11). Existing indices are always allowed.
                Self::reject_if_over_cap(&state.pending_tool_calls, index)?;
                let entry = state.pending_tool_calls.entry(index).or_default();
                entry.args_buf.push_str(&args_chunk);
                events.push(ProcessorEvent::PartDelta {
                    kind: DeltaKind::ToolArgs,
                    delta: args_chunk,
                });
            }
            ChatDelta::Finish { reason, usage } => {
                state.usage = usage.or(state.usage);
                state.finish = Some(reason);
                flush_into_parts(&mut state, &mut parts)?;
            }
        }

        Ok(StepOutcome {
            parts,
            events,
            next: state,
        })
    }
}

/// Drain the accumulator into terminal parts. Called on `Finish`.
///
/// VALIDATE-ALL-THEN-DRAIN. Every pending tool call is validated
/// (non-empty name, unique `call_id` across indices, `args_buf` parses as
/// JSON — empty buffer treated as `{}` for zero-param tools) BEFORE any
/// `state` field is drained. If any validation fails we return `Err` WITHOUT
/// having drained `current_reasoning` / `current_text`, so the reasoning and
/// text are preserved in `state` rather than being lost on
/// a malformed tool-call stream.
fn flush_into_parts(
    state: &mut ProcessorState,
    parts: &mut Vec<ProcessorPart>,
) -> Result<(), ProviderError> {
    // Take the pending tool calls FIRST. This only moves the pending map; it
    // does NOT touch reasoning/text, so an early return below still leaves
    // those accumulators intact in `state`.
    let pending = std::mem::take(&mut state.pending_tool_calls);

    // Pre-validate + fully materialize every tool-call part up front. Nothing
    // here mutates reasoning/text state, so any `Err` return is side-effect
    // free with respect to the preserved reasoning/text fields.
    let mut seen_call_ids: std::collections::HashSet<String> =
        std::collections::HashSet::with_capacity(pending.len());
    let mut tool_parts: Vec<ProcessorPart> = Vec::with_capacity(pending.len());
    for (idx, call) in pending {
        if call.name.is_empty() {
            return Err(ProviderError::Decode(format!(
                "tool_call index {idx} finished without a name"
            )));
        }
        // Duplicate call_id across DIFFERENT indices would clobber tool
        // results downstream (results are keyed by call_id). Reject up front.
        if !seen_call_ids.insert(call.call_id.clone()) {
            return Err(ProviderError::Decode(format!(
                "duplicate call_id across tool indices (call_id={})",
                call.call_id
            )));
        }
        let args_str = if call.args_buf.is_empty() {
            "{}".to_string()
        } else {
            call.args_buf
        };
        let args: serde_json::Value = serde_json::from_str(&args_str).map_err(|e| {
            ProviderError::Decode(format!(
                "tool_call args invalid JSON (call_id={}, name={}): {e}",
                call.call_id, call.name
            ))
        })?;
        tool_parts.push(ProcessorPart::ToolCall {
            call_id: call.call_id,
            name: call.name,
            args,
        });
    }

    // All validations passed — NOW it is safe to drain reasoning/text and emit
    // every part in the canonical order: reasoning, text, tool calls, finish.
    if !state.current_reasoning.is_empty() {
        parts.push(ProcessorPart::Reasoning {
            text: std::mem::take(&mut state.current_reasoning),
            signature: state.current_reasoning_signature.take(),
        });
    }
    if !state.current_text.is_empty() {
        parts.push(ProcessorPart::Text {
            text: std::mem::take(&mut state.current_text),
        });
    }
    parts.extend(tool_parts);
    if let Some(reason) = state.finish {
        parts.push(ProcessorPart::StepFinish {
            reason: finish_reason_label(reason).to_string(),
            usage: state.usage.clone(),
        });
    }
    Ok(())
}

fn finish_reason_label(reason: FinishReason) -> &'static str {
    match reason {
        FinishReason::EndTurn => "end_turn",
        FinishReason::ToolUse => "tool_use",
        FinishReason::MaxTokens => "max_tokens",
        FinishReason::MaxSteps => "max_steps",
        FinishReason::Length => "length",
        FinishReason::ContentFilter => "content_filter",
        FinishReason::Error => "error",
        FinishReason::Cancelled => "cancelled",
        FinishReason::Halted => "halted",
    }
}

#[cfg(test)]
mod flush_validate_then_drain_tests {
    //! `flush_into_parts` validate-all-then-drain.
    //!
    //! Two guarantees:
    //! 1. A `call_id` reused across DIFFERENT tool-call indices is a
    //!    clean `ProviderError::Decode` (it would otherwise clobber downstream
    //!    tool-result routing, which keys results by `call_id`).
    //! 2. On ANY validation error, `current_reasoning` / `current_text`
    //!    are NOT drained — they remain in `state` so the reasoning is not lost
    //!    on a malformed tool-call stream.
    use super::*;

    fn pending(call_id: &str, name: &str, args: &str) -> PendingToolCall {
        PendingToolCall {
            call_id: call_id.to_string(),
            name: name.to_string(),
            args_buf: args.to_string(),
        }
    }

    #[test]
    fn duplicate_call_id_across_indices_errors_and_preserves_reasoning() {
        let mut state = ProcessorState {
            current_reasoning: "important chain of thought".to_string(),
            current_text: "partial answer".to_string(),
            ..Default::default()
        };
        // Two DISTINCT indices sharing the same call_id.
        state
            .pending_tool_calls
            .insert(0, pending("dup", "bash", "{}"));
        state
            .pending_tool_calls
            .insert(1, pending("dup", "read_file", "{}"));

        let mut parts = Vec::new();
        let err = flush_into_parts(&mut state, &mut parts)
            .expect_err("duplicate call_id across indices must error");

        match err {
            ProviderError::Decode(msg) => {
                assert!(
                    msg.contains("duplicate call_id"),
                    "unexpected decode message: {msg}"
                );
            }
            other => panic!("expected Decode error; got {other:?}"),
        }

        // reasoning + text NOT drained — still present in state.
        assert_eq!(state.current_reasoning, "important chain of thought");
        assert_eq!(state.current_text, "partial answer");
        // Nothing emitted on the error path.
        assert!(parts.is_empty(), "no parts should be drained on error");
    }

    #[test]
    fn missing_name_errors_and_preserves_reasoning() {
        let mut state = ProcessorState {
            current_reasoning: "thinking".to_string(),
            ..Default::default()
        };
        // Empty name → validation failure.
        state.pending_tool_calls.insert(0, pending("c1", "", "{}"));

        let mut parts = Vec::new();
        let err = flush_into_parts(&mut state, &mut parts).expect_err("empty name must error");
        assert!(matches!(err, ProviderError::Decode(_)));
        // reasoning preserved.
        assert_eq!(state.current_reasoning, "thinking");
        assert!(parts.is_empty());
    }

    #[test]
    fn invalid_args_json_errors_and_preserves_reasoning() {
        let mut state = ProcessorState {
            current_reasoning: "thinking".to_string(),
            ..Default::default()
        };
        state
            .pending_tool_calls
            .insert(0, pending("c1", "bash", "{not-json"));

        let mut parts = Vec::new();
        let err =
            flush_into_parts(&mut state, &mut parts).expect_err("invalid args JSON must error");
        assert!(matches!(err, ProviderError::Decode(_)));
        // reasoning preserved even though the failure is the LAST validation.
        assert_eq!(state.current_reasoning, "thinking");
        assert!(parts.is_empty());
    }

    #[test]
    fn valid_calls_drain_in_canonical_order() {
        let mut state = ProcessorState {
            current_reasoning: "r".to_string(),
            current_text: "t".to_string(),
            finish: Some(FinishReason::ToolUse),
            ..Default::default()
        };
        state
            .pending_tool_calls
            .insert(0, pending("a", "bash", "{}"));
        state
            .pending_tool_calls
            .insert(1, pending("b", "read_file", "{}"));

        let mut parts = Vec::new();
        flush_into_parts(&mut state, &mut parts).expect("valid flush");

        // reasoning, text, tool(a), tool(b), step_finish
        assert!(matches!(&parts[0], ProcessorPart::Reasoning { text, .. } if text == "r"));
        assert!(matches!(&parts[1], ProcessorPart::Text { text } if text == "t"));
        assert!(matches!(&parts[2], ProcessorPart::ToolCall { call_id, .. } if call_id == "a"));
        assert!(matches!(&parts[3], ProcessorPart::ToolCall { call_id, .. } if call_id == "b"));
        assert!(matches!(&parts[4], ProcessorPart::StepFinish { .. }));
        // Drained.
        assert!(state.current_reasoning.is_empty());
        assert!(state.current_text.is_empty());
        assert!(state.pending_tool_calls.is_empty());
    }
}

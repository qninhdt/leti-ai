//! Compaction — context-pressure-driven summarization as a loop step.
//!
//! Triggered at the top of each `run_loop` iteration when projected token
//! count exceeds `agent.context_window * agent.compaction_threshold`. A
//! synthetic user message asks the model to summarize older messages; the
//! resulting assistant text is stored as `Part::Compaction` and the
//! projection layer substitutes it for the listed `compacted_message_ids`.
//!
//! Design notes (from cross-check):
//! - Overflow is detected as a precondition to the next turn rather
//!   than a flag set on the previous one.
//! - A mechanical (no-LLM) summary fallback is available. We use the LLM
//!   by default; the mechanical path is a robustness add-on.
//! - We keep `PRESERVE_RECENT = 4` total messages. Conservative for short
//!   multi-tool turns.

use std::sync::Arc;

use crate::adapters::event_sink::EventSink;
use crate::adapters::memory_store::MemoryStore;
use crate::agent::AgentDefinition;
use crate::error::CoreError;
use crate::projection::LlmMessage;
use crate::runtime::persist::{append_message_with_event, append_part_with_event};
use crate::runtime::token_estimate::estimate_conversation_tokens;
use crate::types::message::{Message, MessageId, Role};
use crate::types::part::{Part, PartId};
use crate::types::session::SessionId;

/// Number of most-recent messages preserved verbatim (never compacted).
pub const PRESERVE_RECENT: usize = 4;

/// Synthetic user prompt asking the model to summarize older messages.
/// Phrased to preserve goal/decisions/files while dropping tool-output
/// bodies.
pub const COMPACTION_REQUEST: &str = "Summarize the conversation history above. Preserve:\n\
- The user's overall goal\n\
- Key decisions and constraints established\n\
- Files read or modified (paths only)\n\
- Tool errors encountered and resolutions\nDrop:\n\
- Verbose tool output bodies\n\
- Code snippets superseded by later edits\n\
- Idle chatter\n\
Output format: bullet points under headers (Goal, Decisions, Files, Errors).\n\
Limit: 500 words.";

/// Decision returned by `should_compact`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactDecision {
    /// Don't compact — projected tokens below threshold.
    Skip,
    /// Compact: trim everything older than the most recent `keep` messages.
    Run { keep: usize },
}

/// Inspect a projection and decide whether to compact.
///
/// Threshold: `agent.context_window * agent.compaction_threshold`. When
/// `provider_actual` is `Some` it overrides the heuristic — this is the
/// path taken after the first turn returns `usage.prompt_tokens`.
#[must_use]
pub fn should_compact(
    msgs: &[LlmMessage],
    agent: &AgentDefinition,
    provider_actual: Option<usize>,
) -> CompactDecision {
    let total = provider_actual.unwrap_or_else(|| estimate_conversation_tokens(msgs));
    // Defense in depth. `AgentDefinition::validate` rejects an invalid
    // threshold at load time; this guard keeps the runtime safe even if an
    // unvalidated definition reaches here. NaN or a non-positive threshold
    // must NOT silently compact-every-turn (a <= 0.0 threshold yields
    // `limit = 0`, and `total < 0` is never true → every turn would compact),
    // so skip (and log) — never-compact is the safe failure mode vs. an
    // infinite compaction loop.
    let threshold = agent.compaction_threshold;
    if threshold.is_nan() || threshold <= 0.0 {
        tracing::error!(
            agent = %agent.slug.as_str(),
            threshold,
            "compaction_threshold is NaN or non-positive — skipping compaction (validator bug upstream)"
        );
        return CompactDecision::Skip;
    }
    let threshold = threshold.clamp(0.0, 1.0);
    let limit = (f64::from(agent.context_window) * f64::from(threshold)) as usize;
    if total < limit {
        return CompactDecision::Skip;
    }
    let keep = PRESERVE_RECENT.min(msgs.len());
    CompactDecision::Run { keep }
}

/// Persist a synthetic user message asking for compaction. Marked as
/// `synthetic` via the message metadata path once that exists; today we
/// just append a plain user message — the COMPACTION_REQUEST text is
/// distinctive enough to be filterable.
pub async fn append_synthetic_request(
    memory: &Arc<dyn MemoryStore>,
    events: &Arc<dyn EventSink>,
    session_id: SessionId,
) -> Result<MessageId, CoreError> {
    let mid = append_message_with_event(memory, events, session_id, Role::User).await?;
    let part = Part::Text {
        id: PartId::new(),
        text: COMPACTION_REQUEST.to_owned(),
    };
    memory.append_part(mid, part).await?;
    Ok(mid)
}

/// Build the projection used for the compaction turn: drop the assistant
/// reasoning, append a synthetic user turn requesting the summary, and
/// keep only the last `keep` real messages alongside the summarization
/// instruction. The full message log stays intact in storage; this is the
/// *projection* the model sees for the compaction call.
#[must_use]
pub fn build_compaction_projection(full: &[LlmMessage], keep: usize) -> Vec<LlmMessage> {
    use crate::projection::LlmRole;
    let mut out = Vec::with_capacity(full.len() + 2);
    if let Some(sys) = full.iter().find(|m| matches!(m.role, LlmRole::System)) {
        out.push(sys.clone());
    }
    let body: Vec<&LlmMessage> = full
        .iter()
        .filter(|m| !matches!(m.role, LlmRole::System))
        .collect();
    let request = LlmMessage {
        role: LlmRole::User,
        content: COMPACTION_REQUEST.to_owned(),
        reasoning: None,
        tool_calls: Vec::new(),
        tool_call_id: None,
    };
    if body.len() > keep {
        let split = body.len() - keep;
        for m in &body[..split] {
            out.push((*m).clone());
        }
        // The summarization request goes after the older block so the
        // summarizer has the context above it before the instruction.
        out.push(request);
        for m in &body[split..] {
            out.push((*m).clone());
        }
    } else {
        // Defense-in-depth: should_compact already returned Skip in this
        // shape, but guarantee we never run a compaction turn without an
        // explicit summarization instruction. Without this, the model
        // produces unrelated text that gets stored as the summary.
        for m in &body {
            out.push((*m).clone());
        }
        out.push(request);
    }
    out
}

/// Persist the compaction summary as a `Part::Compaction` on a freshly
/// created assistant message. `superseded` lists the message IDs the
/// summary replaces; the projection layer substitutes the summary in
/// their place on subsequent turns.
pub async fn append_compaction_part(
    memory: &Arc<dyn MemoryStore>,
    events: &Arc<dyn EventSink>,
    session_id: SessionId,
    summary: String,
    superseded: Vec<MessageId>,
    original_token_count: u32,
) -> Result<MessageId, CoreError> {
    let mid = append_message_with_event(memory, events, session_id, Role::Assistant).await?;
    let part_id = PartId::new();
    let part = Part::Compaction {
        id: part_id,
        summary,
        compacted_message_ids: superseded
            .iter()
            .map(|m| m.0.to_string())
            .collect::<Vec<_>>(),
        original_token_count,
    };
    append_part_with_event(memory, events, session_id, mid, part).await?;
    Ok(mid)
}

/// Identify the message IDs that a compaction would supersede. Drops the
/// last `keep` non-system messages and the system message itself.
pub fn superseded_messages(msgs: &[Message], keep: usize) -> Vec<MessageId> {
    let body: Vec<&Message> = msgs.iter().filter(|m| m.role != Role::System).collect();
    if body.len() <= keep {
        return Vec::new();
    }
    let split = body.len() - keep;
    body[..split].iter().map(|m| m.id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{AgentDefinition, AgentSlug, PromptSegments};
    use crate::projection::{LlmMessage, LlmRole};

    fn agent() -> AgentDefinition {
        AgentDefinition {
            slug: AgentSlug::new("general").unwrap(),
            title: "General".into(),
            description: String::new(),
            prompt_segments: Some(PromptSegments::default()),
            tool_allowlist: Vec::new(),
            model_id: "test/model".into(),
            default_temperature: 0.0,
            context_window: 1000,
            compaction_threshold: 0.8,
            compaction_summary_cap_tokens: 500,
            hidden: false,
        }
    }

    fn msg(role: LlmRole, body: &str) -> LlmMessage {
        LlmMessage {
            role,
            content: body.to_string(),
            reasoning: None,
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    #[test]
    fn skips_when_under_threshold() {
        let convo = vec![msg(LlmRole::User, "hello")];
        let d = should_compact(&convo, &agent(), None);
        assert_eq!(d, CompactDecision::Skip);
    }

    #[test]
    fn fires_at_threshold_via_provider_actual() {
        let convo = vec![msg(LlmRole::User, "hi")];
        // ctx 1000 * 0.8 = 800 — provider says we're at 850.
        let d = should_compact(&convo, &agent(), Some(850));
        assert!(matches!(d, CompactDecision::Run { keep: 1 }));
    }

    #[test]
    fn fires_via_heuristic() {
        // 4000 chars / 4 = 1000 tokens, threshold 800 → fire.
        let big = "x".repeat(4000);
        let convo = vec![msg(LlmRole::User, &big)];
        let d = should_compact(&convo, &agent(), None);
        assert!(matches!(d, CompactDecision::Run { .. }));
    }

    #[test]
    fn superseded_drops_oldest_excludes_recent_keeps_system() {
        use crate::types::message::{Message, MessageId, Role};
        use crate::types::session::SessionId;
        let sid = SessionId::new();
        let mk = |role: Role| Message {
            id: MessageId::new(),
            session_id: sid,
            role,
            created_at: chrono::Utc::now(),
        };
        let sys = mk(Role::System);
        let u0 = mk(Role::User);
        let a0 = mk(Role::Assistant);
        let u1 = mk(Role::User);
        let a1 = mk(Role::Assistant);
        let msgs = vec![sys.clone(), u0.clone(), a0.clone(), u1.clone(), a1.clone()];
        // keep=2 -> non-system body has 4 entries; supersede [u0, a0].
        let s = superseded_messages(&msgs, 2);
        assert_eq!(s, vec![u0.id, a0.id]);
        // keep>=body.len() -> nothing superseded.
        let none = superseded_messages(&msgs, 4);
        assert!(none.is_empty());
        let none2 = superseded_messages(&msgs, 99);
        assert!(none2.is_empty());
    }

    #[test]
    fn provider_actual_overrides_heuristic_regardless_of_message_size() {
        // Tiny convo + huge provider_actual → fires.
        let convo = vec![msg(LlmRole::User, "hi")];
        let d = should_compact(&convo, &agent(), Some(10_000));
        assert!(matches!(d, CompactDecision::Run { .. }));

        // Huge convo + tiny provider_actual → skips. Provider value
        // anchors the decision; heuristic is the fallback.
        let big = msg(LlmRole::User, &"x".repeat(8000));
        let d = should_compact(&[big], &agent(), Some(10));
        assert_eq!(d, CompactDecision::Skip);
    }

    #[test]
    fn keep_clamps_to_message_count_when_below_preserve_recent() {
        // PRESERVE_RECENT = 4, but only 2 messages — Run.keep must
        // clamp to msgs.len() rather than report 4 phantom slots.
        let convo = vec![
            msg(LlmRole::User, "first"),
            msg(LlmRole::User, &"x".repeat(8000)),
        ];
        let d = should_compact(&convo, &agent(), Some(900));
        if let CompactDecision::Run { keep } = d {
            assert_eq!(keep, 2, "keep clamps to msgs.len() when < PRESERVE_RECENT");
        } else {
            panic!("expected Run, got {d:?}");
        }
    }

    #[test]
    fn build_compaction_projection_preserves_system_and_appends_request() {
        let convo = vec![
            msg(LlmRole::System, "you are an assistant"),
            msg(LlmRole::User, "old1"),
            msg(LlmRole::Assistant, "ans1"),
            msg(LlmRole::User, "recent"),
        ];
        let proj = build_compaction_projection(&convo, 1);
        // First message must be the system prompt.
        assert!(matches!(proj[0].role, LlmRole::System));
        // The COMPACTION_REQUEST must appear somewhere.
        assert!(proj.iter().any(|m| m.content == COMPACTION_REQUEST));
        // The most-recent message (kept) must appear after the request.
        let req_idx = proj
            .iter()
            .position(|m| m.content == COMPACTION_REQUEST)
            .unwrap();
        let recent_idx = proj.iter().rposition(|m| m.content == "recent").unwrap();
        assert!(
            req_idx < recent_idx,
            "kept tail must follow the compaction request"
        );
    }
}

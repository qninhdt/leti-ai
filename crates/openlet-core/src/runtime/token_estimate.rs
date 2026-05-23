//! Token estimator — bytes/4 heuristic + provider-actual override.
//!
//! Phase-07 deliberately picks the cheap heuristic (matches claw-code's
//! `estimate_message_tokens` at `compact.rs:448-462`) to avoid pulling
//! `tiktoken-rs` in Phase 7. When a provider supplies `usage.prompt_tokens`
//! from the previous turn, that value anchors the next estimate so drift
//! stays bounded. Phase-08 may upgrade to `tiktoken-rs` if accuracy proves
//! insufficient — the trait shape is stable.

use crate::projection::{LlmMessage, LlmRole};

/// Rough chars-per-token used by the heuristic. 4 is the OpenAI tokenizer
/// rule of thumb across English + code; claw-code uses the same constant.
pub const CHARS_PER_TOKEN: usize = 4;

/// Estimate tokens in a single message body. Counts text + reasoning +
/// tool-call args + tool result bodies. Always returns at least 1 so empty
/// messages still register.
#[must_use]
pub fn estimate_message_tokens(msg: &LlmMessage) -> usize {
    let mut chars = msg.content.len();
    if let Some(r) = &msg.reasoning {
        chars += r.len();
    }
    for c in &msg.tool_calls {
        chars += c.name.len() + c.args_json.len();
    }
    if matches!(msg.role, LlmRole::Tool) {
        // tool results: id is small; content already counted above
    }
    (chars / CHARS_PER_TOKEN).max(1)
}

/// Estimate tokens across a projected conversation. Cheap O(N) walk.
#[must_use]
pub fn estimate_conversation_tokens(msgs: &[LlmMessage]) -> usize {
    msgs.iter().map(estimate_message_tokens).sum()
}

/// Provider-anchored estimate. When `provider_actual` is `Some`, use it
/// directly — it's authoritative for the conversation up to that point.
/// The caller can add the unsent tail's heuristic estimate on top.
#[must_use]
pub fn anchored_estimate(provider_actual: Option<usize>, msgs: &[LlmMessage]) -> usize {
    match provider_actual {
        Some(actual) => actual,
        None => estimate_conversation_tokens(msgs),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection::{LlmMessage, LlmRole};

    fn user(content: &str) -> LlmMessage {
        LlmMessage {
            role: LlmRole::User,
            content: content.to_string(),
            reasoning: None,
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    #[test]
    fn empty_message_still_counts_one() {
        assert_eq!(estimate_message_tokens(&user("")), 1);
    }

    #[test]
    fn bytes_div_four() {
        let m = user(&"x".repeat(400));
        assert_eq!(estimate_message_tokens(&m), 100);
    }

    #[test]
    fn provider_anchor_overrides() {
        let convo = vec![user(&"y".repeat(400))];
        assert_eq!(anchored_estimate(Some(42), &convo), 42);
        assert_eq!(anchored_estimate(None, &convo), 100);
    }
}

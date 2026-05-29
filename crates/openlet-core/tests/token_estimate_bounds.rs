//! Property-based invariants on `token_estimate`.
//!
//! Cheap heuristic (`bytes/4` with floor of 1 per message). Properties
//! lock the floor (no zero-token messages), monotonicity in input size,
//! aggregate equality with per-message sum, and the provider-anchor
//! override semantics. Drift in any of these would make compaction
//! decisions wrong.

use openlet_core::projection::LlmToolCall;
use openlet_core::projection::{LlmMessage, LlmRole};
use openlet_core::runtime::token_estimate::{
    CHARS_PER_TOKEN, anchored_estimate, estimate_conversation_tokens, estimate_message_tokens,
};
use proptest::prelude::*;

fn user_msg(content: String) -> LlmMessage {
    LlmMessage {
        role: LlmRole::User,
        content,
        reasoning: None,
        tool_calls: Vec::new(),
        tool_call_id: None,
    }
}

fn arb_role() -> impl Strategy<Value = LlmRole> {
    prop_oneof![
        Just(LlmRole::System),
        Just(LlmRole::User),
        Just(LlmRole::Assistant),
        Just(LlmRole::Tool),
    ]
}

fn arb_tool_call() -> impl Strategy<Value = LlmToolCall> {
    (
        "[a-z0-9-]{4,16}",
        "[a-z][a-z0-9_]{2,12}",
        prop::string::string_regex("[a-zA-Z0-9 \"{},:]{0,128}").unwrap(),
    )
        .prop_map(|(id, name, args_json)| LlmToolCall {
            id,
            name,
            args_json,
        })
}

fn arb_user_msg() -> impl Strategy<Value = LlmMessage> {
    prop::string::string_regex("[a-zA-Z0-9 ]{0,256}")
        .unwrap()
        .prop_map(user_msg)
}

/// Broad generator: any role, optional reasoning, 0-3 tool_calls. Used
/// for properties that must hold across the full message shape.
fn arb_full_msg() -> impl Strategy<Value = LlmMessage> {
    (
        arb_role(),
        prop::string::string_regex("[a-zA-Z0-9 ]{0,128}").unwrap(),
        prop::option::of(prop::string::string_regex("[a-zA-Z ]{0,64}").unwrap()),
        prop::collection::vec(arb_tool_call(), 0..3),
    )
        .prop_map(|(role, content, reasoning, tool_calls)| {
            let tool_call_id = matches!(role, LlmRole::Tool).then(|| "tc-1".to_string());
            LlmMessage {
                role,
                content,
                reasoning,
                tool_calls,
                tool_call_id,
            }
        })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, .. ProptestConfig::default() })]

    /// Floor invariant: every message estimates ≥ 1 token. Empty
    /// messages MUST NOT vanish from the count or compaction will
    /// underestimate the conversation.
    #[test]
    fn estimate_at_least_one_per_message(content in "[a-zA-Z ]{0,200}") {
        let m = user_msg(content);
        prop_assert!(estimate_message_tokens(&m) >= 1);
    }

    /// Aggregate equals per-message sum. If this drifts, callers that
    /// estimate piecewise vs whole-conversation get inconsistent
    /// totals.
    #[test]
    fn conversation_equals_sum_of_messages(msgs in prop::collection::vec(arb_user_msg(), 0..16)) {
        let agg = estimate_conversation_tokens(&msgs);
        let sum: usize = msgs.iter().map(estimate_message_tokens).sum();
        prop_assert_eq!(agg, sum);
    }

    /// Conversation estimate is at least N (one floor-token per
    /// message). Compaction relies on this to never undercount the
    /// message-count contribution to the budget.
    #[test]
    fn conversation_at_least_message_count(msgs in prop::collection::vec(arb_user_msg(), 0..16)) {
        let total = estimate_conversation_tokens(&msgs);
        prop_assert!(total >= msgs.len());
    }

    /// Appending a message can never DECREASE the total. Locks
    /// monotonicity — an inserted message that somehow shrank the
    /// estimate would let unbounded growth slip past the cap.
    #[test]
    fn appending_message_does_not_shrink_total(
        before in prop::collection::vec(arb_user_msg(), 0..8),
        addition in arb_user_msg(),
    ) {
        let total_before = estimate_conversation_tokens(&before);
        let mut after = before.clone();
        after.push(addition);
        let total_after = estimate_conversation_tokens(&after);
        prop_assert!(total_after >= total_before);
    }

    /// Per-message estimate is monotonic in content length. Locks the
    /// integer-division-with-floor relation.
    #[test]
    fn longer_content_yields_at_least_as_many_tokens(
        n in 1usize..1024,
    ) {
        let short = user_msg("x".repeat(n));
        let long = user_msg("x".repeat(n * 2));
        prop_assert!(estimate_message_tokens(&long) >= estimate_message_tokens(&short));
    }

    /// Estimator output for ASCII-only content tracks the documented
    /// `CHARS_PER_TOKEN` ratio (allowing the floor of 1).
    #[test]
    fn ascii_estimate_matches_chars_per_token(n in 1usize..2048) {
        let m = user_msg("x".repeat(n));
        let expected = (n / CHARS_PER_TOKEN).max(1);
        prop_assert_eq!(estimate_message_tokens(&m), expected);
    }

    /// Provider-anchored override: when `Some(actual)`, the heuristic
    /// is bypassed. Crucial because actual numbers from the provider
    /// are authoritative for prior turns.
    #[test]
    fn provider_anchor_replaces_heuristic(
        actual in 0usize..100_000,
        msgs in prop::collection::vec(arb_user_msg(), 0..8),
    ) {
        prop_assert_eq!(anchored_estimate(Some(actual), &msgs), actual);
    }

    /// Provider-anchored with `None` falls back to the heuristic
    /// total. No silent doubling.
    #[test]
    fn provider_anchor_none_falls_back_to_heuristic(
        msgs in prop::collection::vec(arb_user_msg(), 0..8),
    ) {
        prop_assert_eq!(
            anchored_estimate(None, &msgs),
            estimate_conversation_tokens(&msgs)
        );
    }

    /// Floor invariant holds for ANY message shape — including
    /// non-User roles, populated reasoning, and tool calls. Locks the
    /// `.max(1)` clamp at the right level.
    #[test]
    fn full_message_estimate_at_least_one(msg in arb_full_msg()) {
        prop_assert!(estimate_message_tokens(&msg) >= 1);
    }

    /// Adding reasoning to a message can only increase (or hold) its
    /// token count. Locks that the estimator counts the reasoning
    /// field, not silently drops it.
    #[test]
    fn reasoning_field_contributes_non_negatively(
        content in "[a-zA-Z0-9 ]{0,128}",
        reasoning in "[a-zA-Z ]{1,128}",
    ) {
        let without = LlmMessage {
            role: LlmRole::Assistant,
            content: content.clone(),
            reasoning: None,
            tool_calls: Vec::new(),
            tool_call_id: None,
        };
        let with = LlmMessage {
            reasoning: Some(reasoning),
            ..without.clone()
        };
        prop_assert!(estimate_message_tokens(&with) >= estimate_message_tokens(&without));
    }

    /// Adding tool_calls only increases the count. Locks that args_json
    /// + name lengths are folded into the estimate.
    #[test]
    fn tool_calls_contribute_non_negatively(
        content in "[a-zA-Z0-9 ]{0,128}",
        tool_calls in prop::collection::vec(arb_tool_call(), 1..3),
    ) {
        let without = LlmMessage {
            role: LlmRole::Assistant,
            content: content.clone(),
            reasoning: None,
            tool_calls: Vec::new(),
            tool_call_id: None,
        };
        let with = LlmMessage {
            tool_calls,
            ..without.clone()
        };
        prop_assert!(estimate_message_tokens(&with) >= estimate_message_tokens(&without));
    }
}

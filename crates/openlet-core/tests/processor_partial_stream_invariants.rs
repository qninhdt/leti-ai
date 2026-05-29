//! Property-based invariants on `Processor::step` over streaming chunks.
//!
//! These properties anchor the processor's contract independent of any
//! specific chunk shape — the test author authors the property, proptest
//! generates the inputs.
//!
//! Covers: streaming-split equivalence with one-shot input, empty-args
//! canonicalization to `{}`, usage-token round-trip on Finish.

use openlet_core::adapters::model_provider::{ChatDelta, FinishReason};
use openlet_core::error::ProviderError;
use openlet_core::runtime::processor::{Processor, ProcessorPart, ProcessorState};
use openlet_core::types::event::Usage;
use proptest::prelude::*;

fn drive(deltas: Vec<ChatDelta>) -> Result<Vec<ProcessorPart>, ProviderError> {
    let mut state = ProcessorState::default();
    let mut all_parts = Vec::new();
    for d in deltas {
        let outcome = Processor::step(state, d)?;
        all_parts.extend(outcome.parts);
        state = outcome.next;
    }
    Ok(all_parts)
}

fn arb_text_chunks() -> impl Strategy<Value = Vec<String>> {
    // 1..20 ASCII text fragments, length 0..32 each.
    prop::collection::vec(
        prop::string::string_regex("[a-zA-Z0-9 ]{0,32}").unwrap(),
        1..20,
    )
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, .. ProptestConfig::default() })]

    /// Property: feeding the same total text as N small content deltas
    /// produces the same concatenated text Part as a single content
    /// delta with all N strings joined. Streaming split is invisible to
    /// the final part.
    #[test]
    fn content_split_invariant_concat_equals_whole(chunks in arb_text_chunks()) {
        let whole: String = chunks.concat();

        // Streamed: one Content per chunk.
        let mut streamed = vec![ChatDelta::Role];
        for c in &chunks {
            streamed.push(ChatDelta::Content { text: c.clone() });
        }
        streamed.push(ChatDelta::Finish {
            reason: FinishReason::EndTurn,
            usage: None,
        });

        // One-shot.
        let one_shot = vec![
            ChatDelta::Role,
            ChatDelta::Content { text: whole.clone() },
            ChatDelta::Finish {
                reason: FinishReason::EndTurn,
                usage: None,
            },
        ];

        let parts_streamed = drive(streamed).expect("streamed drive");
        let parts_oneshot = drive(one_shot).expect("oneshot drive");

        let text_streamed: Option<String> = parts_streamed.iter().find_map(|p| {
            if let ProcessorPart::Text { text } = p {
                Some(text.clone())
            } else {
                None
            }
        });
        let text_oneshot: Option<String> = parts_oneshot.iter().find_map(|p| {
            if let ProcessorPart::Text { text } = p {
                Some(text.clone())
            } else {
                None
            }
        });

        // If the whole string is empty, the processor may or may not emit
        // a Text part — either is acceptable, but if both emit, contents
        // must match. If only one emits, the other must be empty-string.
        match (text_streamed, text_oneshot) {
            (Some(a), Some(b)) => prop_assert_eq!(a, b),
            (None, None) => {}
            (Some(s), None) | (None, Some(s)) => prop_assert!(s.is_empty()),
        }
    }

    /// Property: an empty args buffer for a tool call always yields the
    /// JSON object `{}` after Finish. Pinned at `processor_malformed_chunks::empty_args_buffer_treated_as_empty_object`
    /// — proptest extends with arbitrary tool names and call_ids.
    #[test]
    fn empty_args_buffer_yields_empty_json_object(
        name in "[a-z][a-z0-9_]{0,15}",
        call_id in "[a-z0-9-]{4,20}",
    ) {
        let deltas = vec![
            ChatDelta::Role,
            ChatDelta::ToolCallStart {
                call_id: call_id.clone(),
                name: name.clone(),
                index: 0,
            },
            ChatDelta::Finish {
                reason: FinishReason::ToolUse,
                usage: None,
            },
        ];
        let parts = drive(deltas).expect("drive");
        let saw_empty_object = parts.iter().any(|p| matches!(p,
            ProcessorPart::ToolCall { args, name: n, .. }
                if n == &name && args.as_object().is_some_and(|o| o.is_empty())
        ));
        prop_assert!(saw_empty_object,
            "expected ToolCall with empty-object args for tool {}, got {:?}",
            name, parts
        );
    }

    /// Property: usage tokens declared in the Finish frame round-trip
    /// untouched into the StepFinish part. No silent normalization.
    #[test]
    fn finish_usage_tokens_round_trip(
        input_tokens in 0u64..1_000_000,
        output_tokens in 0u64..1_000_000,
    ) {
        let deltas = vec![
            ChatDelta::Role,
            ChatDelta::Content { text: "x".into() },
            ChatDelta::Finish {
                reason: FinishReason::EndTurn,
                usage: Some(Usage {
                    input_tokens,
                    output_tokens,
                    ..Default::default()
                }),
            },
        ];
        let parts = drive(deltas).expect("drive");
        let usage_part = parts.iter().find_map(|p| match p {
            ProcessorPart::StepFinish { usage: Some(u), .. } => Some(u.clone()),
            _ => None,
        }).expect("StepFinish usage present");
        prop_assert_eq!(usage_part.input_tokens, input_tokens);
        prop_assert_eq!(usage_part.output_tokens, output_tokens);
    }
}

//! `chunk_decoder::decode_chunk` malformed input handling.
//!
//! Cases:
//! 1. Envelope-level garbage → `Decode("chunk envelope: ...")`
//! 2. Wrong-shape `choices` (string instead of array) → typed Decode
//! 3. `tool_calls[i].function = null` → no panic; emits ToolCallStart
//!    with empty `name` (per fallback path)
//! 4. Unknown `finish_reason` → maps to `FinishReason::Error`
//! 5. `reasoning_content` AND `reasoning` in same chunk → exactly one
//!    `Reasoning` delta emitted (precedence: `reasoning_content`)

use openlet_adapters::openai_compat::chunk_decoder::decode_chunk;
use openlet_core::adapters::model_provider::{ChatDelta, FinishReason};
use openlet_core::error::ProviderError;

#[test]
fn envelope_garbage_returns_decode_error_with_context() {
    let err = decode_chunk("not json").expect_err("must error");
    match err {
        ProviderError::Decode(msg) => {
            assert!(msg.starts_with("chunk envelope:"), "{msg}");
        }
        other => panic!("expected Decode; got {other:?}"),
    }
}

#[test]
fn choices_wrong_shape_returns_decode_error() {
    let err = decode_chunk(r#"{"choices":"oops"}"#).expect_err("must error");
    assert!(matches!(err, ProviderError::Decode(_)));
}

#[test]
fn null_tool_call_function_does_not_panic() {
    // `function: null` is allowed by the Deserialize derive (Option<ChunkFn>).
    // Per the fallback path in `push_tool_delta`, we emit ToolCallStart
    // with empty name and let the processor surface the missing-name
    // error on Finish.
    let p = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_x","function":null}]}}]}"#;
    let deltas = decode_chunk(p).expect("decode");
    let mut saw = false;
    for d in &deltas {
        if let ChatDelta::ToolCallStart {
            call_id,
            name,
            index,
        } = d
        {
            assert_eq!(call_id, "call_x");
            assert_eq!(name, "");
            assert_eq!(*index, 0);
            saw = true;
        }
    }
    assert!(saw, "expected ToolCallStart with empty name fallback");
}

#[test]
fn unknown_finish_reason_maps_to_error_variant() {
    let p = r#"{"choices":[{"delta":{},"finish_reason":"definitely_not_a_real_reason"}]}"#;
    let deltas = decode_chunk(p).expect("decode");
    let last = deltas.last().expect("at least one delta");
    assert!(matches!(
        last,
        ChatDelta::Finish {
            reason: FinishReason::Error,
            ..
        }
    ));
}

#[test]
fn reasoning_content_takes_precedence_over_reasoning_field() {
    // Per chunk_decoder.rs:120 — `reasoning_content.or(reasoning)`.
    // When both fields are present, only `reasoning_content` should
    // surface; the duplicate `reasoning` payload must NOT emit a
    // second Reasoning delta.
    let p = r#"{"choices":[{"delta":{"reasoning_content":"primary","reasoning":"duplicate"}}]}"#;
    let deltas = decode_chunk(p).expect("decode");
    let reasoning_deltas: Vec<_> = deltas
        .iter()
        .filter(|d| matches!(d, ChatDelta::Reasoning { .. }))
        .collect();
    assert_eq!(
        reasoning_deltas.len(),
        1,
        "exactly one Reasoning delta when both fields are present"
    );
    if let ChatDelta::Reasoning { text, .. } = reasoning_deltas[0] {
        assert_eq!(text, "primary", "reasoning_content wins precedence");
    }
}

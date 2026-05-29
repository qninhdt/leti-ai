//! Phase 4 — `Processor::step` rejection of malformed chunk streams.
//!
//! Cases under test:
//!
//! 1. Duplicate `(name, index)` after non-empty buffer → `Decode` with
//!    structured "duplicate tool_call index N" message.
//! 2. Empty `index` name on Finish → `Decode("tool_call index N finished
//!    without a name")`.
//! 3. Args buffer is invalid JSON → `Decode` with call_id + name in the
//!    message.
//! 4. Fill 64 distinct indices, then add a 65th → `Decode("too many
//!    pending tool calls (cap 64)")`. Updating an existing index 0
//!    after the cap still works.
//! 5. Empty args buffer → emits `ProcessorPart::ToolCall { args: {} }`,
//!    not an error.

use openlet_core::adapters::model_provider::{ChatDelta, FinishReason};
use openlet_core::error::ProviderError;
use openlet_core::runtime::processor::{Processor, ProcessorPart, ProcessorState};

fn step(state: ProcessorState, delta: ChatDelta) -> Result<ProcessorState, ProviderError> {
    Processor::step(state, delta).map(|o| o.next)
}

#[test]
fn duplicate_tool_call_index_with_streaming_args_errors() {
    let mut state = ProcessorState::default();
    state = step(
        state,
        ChatDelta::ToolCallStart {
            call_id: "call-a".into(),
            name: "bash".into(),
            index: 0,
        },
    )
    .unwrap();
    state = step(
        state,
        ChatDelta::ToolCallArgsDelta {
            index: 0,
            args_chunk: "{\"cmd".into(),
        },
    )
    .unwrap();
    let err = step(
        state,
        ChatDelta::ToolCallStart {
            call_id: "call-b".into(),
            name: "bash".into(),
            index: 0,
        },
    )
    .expect_err("duplicate index with non-empty args must error");
    match err {
        ProviderError::Decode(msg) => {
            assert!(
                msg.contains("duplicate tool_call index 0"),
                "msg missing 'duplicate tool_call index 0': {msg}"
            );
            assert!(
                msg.contains("call_id=call-a"),
                "msg missing existing call_id: {msg}"
            );
        }
        other => panic!("expected Decode; got {other:?}"),
    }
}

#[test]
fn finish_without_tool_call_name_errors() {
    let mut state = ProcessorState::default();
    state = step(
        state,
        ChatDelta::ToolCallStart {
            call_id: "call-x".into(),
            name: String::new(), // missing on opening chunk
            index: 7,
        },
    )
    .unwrap();
    let err = step(
        state,
        ChatDelta::Finish {
            reason: FinishReason::ToolUse,
            usage: None,
        },
    )
    .expect_err("Finish with empty tool name must error");
    match err {
        ProviderError::Decode(msg) => {
            assert!(
                msg.contains("tool_call index 7 finished without a name"),
                "{msg}"
            );
        }
        other => panic!("expected Decode; got {other:?}"),
    }
}

#[test]
fn invalid_json_args_buffer_errors_with_call_id_and_name() {
    let mut state = ProcessorState::default();
    state = step(
        state,
        ChatDelta::ToolCallStart {
            call_id: "call-z".into(),
            name: "list".into(),
            index: 0,
        },
    )
    .unwrap();
    state = step(
        state,
        ChatDelta::ToolCallArgsDelta {
            index: 0,
            args_chunk: "]not json[".into(),
        },
    )
    .unwrap();
    let err = step(
        state,
        ChatDelta::Finish {
            reason: FinishReason::ToolUse,
            usage: None,
        },
    )
    .expect_err("malformed args must error on Finish");
    match err {
        ProviderError::Decode(msg) => {
            assert!(msg.contains("call_id=call-z"), "{msg}");
            assert!(msg.contains("name=list"), "{msg}");
            assert!(msg.contains("invalid JSON"), "{msg}");
        }
        other => panic!("expected Decode; got {other:?}"),
    }
}

#[test]
fn pending_tool_call_cap_at_64_distinct_indices() {
    // Fill 64 distinct indices. After the cap, a NEW index errors but
    // updating an existing one (index 0) still passes.
    let mut state = ProcessorState::default();
    for i in 0..64 {
        state = step(
            state,
            ChatDelta::ToolCallStart {
                call_id: format!("call-{i}"),
                name: format!("tool-{i}"),
                index: i,
            },
        )
        .unwrap();
    }
    assert_eq!(state.pending_tool_calls.len(), 64);

    let err = step(
        state.clone(),
        ChatDelta::ToolCallStart {
            call_id: "call-65".into(),
            name: "tool-65".into(),
            index: 65,
        },
    )
    .expect_err("65th distinct index must error");
    match err {
        ProviderError::Decode(msg) => {
            assert!(
                msg.contains("too many pending tool calls (cap 64)"),
                "{msg}"
            );
        }
        other => panic!("expected Decode; got {other:?}"),
    }

    // Updating existing index 0 (args delta) still works.
    let _ = step(
        state,
        ChatDelta::ToolCallArgsDelta {
            index: 0,
            args_chunk: "{}".into(),
        },
    )
    .expect("updating existing index after cap must succeed");
}

#[test]
fn empty_args_buffer_treated_as_empty_object() {
    let mut state = ProcessorState::default();
    state = step(
        state,
        ChatDelta::ToolCallStart {
            call_id: "call-y".into(),
            name: "list".into(),
            index: 0,
        },
    )
    .unwrap();
    let outcome = Processor::step(
        state,
        ChatDelta::Finish {
            reason: FinishReason::ToolUse,
            usage: None,
        },
    )
    .expect("Finish with empty args must succeed (treated as {})");
    let mut saw_tool_call = false;
    for p in &outcome.parts {
        if let ProcessorPart::ToolCall { args, name, .. } = p {
            assert_eq!(name, "list");
            assert_eq!(args, &serde_json::json!({}));
            saw_tool_call = true;
        }
    }
    assert!(saw_tool_call, "expected ToolCall part with args = {{}}");
}

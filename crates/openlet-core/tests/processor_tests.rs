//! Table-driven processor tests covering each `ChatDelta` variant +
//! interleavings. Pure logic, no IO.

use openlet_core::adapters::model_provider::{ChatDelta, FinishReason};
use openlet_core::error::ProviderError;
use openlet_core::runtime::processor::{Processor, ProcessorEvent, ProcessorPart, ProcessorState};
use openlet_core::types::event::{DeltaKind, Usage};

fn drive(
    deltas: Vec<ChatDelta>,
) -> Result<(Vec<ProcessorPart>, Vec<ProcessorEvent>), ProviderError> {
    let mut state = ProcessorState::default();
    let mut all_parts = Vec::new();
    let mut all_events = Vec::new();
    for d in deltas {
        let outcome = Processor::step(state, d)?;
        all_parts.extend(outcome.parts);
        all_events.extend(outcome.events);
        state = outcome.next;
    }
    Ok((all_parts, all_events))
}

#[test]
fn pure_text_finish_endturn() {
    let (parts, events) = drive(vec![
        ChatDelta::Role,
        ChatDelta::Content {
            text: "Hello, ".to_string(),
        },
        ChatDelta::Content {
            text: "world".to_string(),
        },
        ChatDelta::Finish {
            reason: FinishReason::EndTurn,
            usage: Some(Usage {
                input_tokens: 10,
                output_tokens: 5,
                ..Default::default()
            }),
        },
    ])
    .unwrap();

    assert_eq!(events.len(), 2);
    assert!(matches!(
        &events[0],
        ProcessorEvent::PartDelta { kind: DeltaKind::Text, delta } if delta == "Hello, "
    ));

    assert!(matches!(
        &parts[0],
        ProcessorPart::Text { text } if text == "Hello, world"
    ));
    assert!(matches!(
        &parts[1],
        ProcessorPart::StepFinish { reason, usage: Some(_) } if reason == "end_turn"
    ));
}

#[test]
fn text_then_single_tool_call() {
    let (parts, _) = drive(vec![
        ChatDelta::Content {
            text: "Looking up...".to_string(),
        },
        ChatDelta::ToolCallStart {
            call_id: "c1".to_string(),
            name: "bash".to_string(),
            index: 0,
        },
        ChatDelta::ToolCallArgsDelta {
            index: 0,
            args_chunk: "{\"cmd\":\"ls\"}".to_string(),
        },
        ChatDelta::Finish {
            reason: FinishReason::ToolUse,
            usage: None,
        },
    ])
    .unwrap();

    assert!(matches!(&parts[0], ProcessorPart::Text { text } if text == "Looking up..."));
    assert!(matches!(
        &parts[1],
        ProcessorPart::ToolCall { call_id, name, args }
            if call_id == "c1" && name == "bash" && args["cmd"] == "ls"
    ));
    assert!(matches!(
        &parts[2],
        ProcessorPart::StepFinish { reason, .. } if reason == "tool_use"
    ));
}

#[test]
fn parallel_tool_calls_indexed() {
    let (parts, _) = drive(vec![
        ChatDelta::ToolCallStart {
            call_id: "a".to_string(),
            name: "bash".to_string(),
            index: 0,
        },
        ChatDelta::ToolCallStart {
            call_id: "b".to_string(),
            name: "read_file".to_string(),
            index: 1,
        },
        ChatDelta::ToolCallArgsDelta {
            index: 0,
            args_chunk: "{\"cmd\":\"".to_string(),
        },
        ChatDelta::ToolCallArgsDelta {
            index: 1,
            args_chunk: "{\"path\":\"/tmp/x\"}".to_string(),
        },
        ChatDelta::ToolCallArgsDelta {
            index: 0,
            args_chunk: "ls\"}".to_string(),
        },
        ChatDelta::Finish {
            reason: FinishReason::ToolUse,
            usage: None,
        },
    ])
    .unwrap();

    let calls: Vec<&ProcessorPart> = parts
        .iter()
        .filter(|p| matches!(p, ProcessorPart::ToolCall { .. }))
        .collect();
    assert_eq!(calls.len(), 2);
}

#[test]
fn reasoning_then_text() {
    let (parts, events) = drive(vec![
        ChatDelta::Reasoning {
            text: "thinking step 1".to_string(),
            signature: None,
        },
        ChatDelta::Content {
            text: "answer".to_string(),
        },
        ChatDelta::Finish {
            reason: FinishReason::EndTurn,
            usage: None,
        },
    ])
    .unwrap();

    assert!(matches!(
        &events[0],
        ProcessorEvent::PartDelta {
            kind: DeltaKind::Reasoning,
            ..
        }
    ));
    assert!(matches!(
        &events[1],
        ProcessorEvent::PartDelta {
            kind: DeltaKind::Text,
            ..
        }
    ));
    // Reasoning emitted before Text in flush order
    assert!(matches!(
        &parts[0],
        ProcessorPart::Reasoning { text, .. } if text == "thinking step 1"
    ));
    assert!(matches!(
        &parts[1],
        ProcessorPart::Text { text } if text == "answer"
    ));
}

#[test]
fn malformed_args_json_fails_at_finish() {
    let res = drive(vec![
        ChatDelta::ToolCallStart {
            call_id: "c1".to_string(),
            name: "bash".to_string(),
            index: 0,
        },
        ChatDelta::ToolCallArgsDelta {
            index: 0,
            args_chunk: "{not-json".to_string(),
        },
        ChatDelta::Finish {
            reason: FinishReason::ToolUse,
            usage: None,
        },
    ]);
    assert!(matches!(res, Err(ProviderError::Decode(_))));
}

#[test]
fn duplicate_tool_call_index_rejected_per_amendment_t() {
    let res = drive(vec![
        ChatDelta::ToolCallStart {
            call_id: "a".to_string(),
            name: "bash".to_string(),
            index: 0,
        },
        ChatDelta::ToolCallArgsDelta {
            index: 0,
            args_chunk: "{".to_string(),
        },
        // Provider tries to reuse index 0 for a different call mid-stream.
        ChatDelta::ToolCallStart {
            call_id: "b".to_string(),
            name: "read_file".to_string(),
            index: 0,
        },
    ]);
    assert!(
        matches!(res, Err(ProviderError::Decode(msg)) if msg.contains("duplicate tool_call index"))
    );
}

#[test]
fn empty_args_buffer_treated_as_empty_object() {
    let (parts, _) = drive(vec![
        ChatDelta::ToolCallStart {
            call_id: "c1".to_string(),
            name: "ping".to_string(),
            index: 0,
        },
        ChatDelta::Finish {
            reason: FinishReason::ToolUse,
            usage: None,
        },
    ])
    .unwrap();

    assert!(matches!(
        &parts[0],
        ProcessorPart::ToolCall { args, .. } if args == &serde_json::json!({})
    ));
}

#[test]
fn finish_without_name_errors() {
    let res = drive(vec![
        ChatDelta::ToolCallArgsDelta {
            index: 0,
            args_chunk: "{}".to_string(),
        },
        ChatDelta::Finish {
            reason: FinishReason::ToolUse,
            usage: None,
        },
    ]);
    assert!(matches!(res, Err(ProviderError::Decode(_))));
}

#[test]
fn role_delta_is_noop() {
    let (parts, events) = drive(vec![
        ChatDelta::Role,
        ChatDelta::Finish {
            reason: FinishReason::EndTurn,
            usage: None,
        },
    ])
    .unwrap();
    // Only step_finish; no events.
    assert!(events.is_empty());
    assert_eq!(parts.len(), 1);
    assert!(matches!(&parts[0], ProcessorPart::StepFinish { .. }));
}

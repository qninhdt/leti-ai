//! Tests for `PartDto::into_part_for_user_input`.
//!
//! Only `Text` and `Reasoning` variants are accepted as user input;
//! all runtime-produced variants (ToolCall, ToolResult, Image, etc.)
//! return `None`.

use openlet_protocol::PartDto;
use uuid::Uuid;

#[test]
fn text_part_converts_to_domain() {
    let dto = PartDto::Text {
        id: Uuid::nil(),
        text: "hello world".into(),
    };

    let part = dto.into_part_for_user_input();
    assert!(part.is_some());

    let part = part.unwrap();
    match part {
        openlet_core::types::part::Part::Text { id, text } => {
            assert_eq!(id.as_uuid(), Uuid::nil());
            assert_eq!(text, "hello world");
        }
        _ => panic!("expected Part::Text"),
    }
}

#[test]
fn reasoning_part_converts_to_domain() {
    let id = Uuid::new_v4();
    let dto = PartDto::Reasoning {
        id,
        text: "thinking...".into(),
    };

    let part = dto.into_part_for_user_input().unwrap();
    match part {
        openlet_core::types::part::Part::Reasoning { id: pid, text } => {
            assert_eq!(pid.as_uuid(), id);
            assert_eq!(text, "thinking...");
        }
        _ => panic!("expected Part::Reasoning"),
    }
}

#[test]
fn tool_call_returns_none() {
    let dto = PartDto::ToolCall {
        id: Uuid::nil(),
        call_id: "call_123".into(),
        name: "bash".into(),
        args: serde_json::json!({"cmd": "ls"}),
    };

    assert!(dto.into_part_for_user_input().is_none());
}

#[test]
fn tool_result_returns_none() {
    let dto = PartDto::ToolResult {
        id: Uuid::nil(),
        call_id: "call_123".into(),
        ok: true,
        text: Some("output".into()),
        error: None,
    };

    assert!(dto.into_part_for_user_input().is_none());
}

#[test]
fn image_returns_none() {
    let dto = PartDto::Image {
        id: Uuid::nil(),
        artifact_id: "art_1".into(),
        mime: "image/png".into(),
        width: 100,
        height: 200,
    };

    assert!(dto.into_part_for_user_input().is_none());
}

#[test]
fn document_returns_none() {
    let dto = PartDto::Document {
        id: Uuid::nil(),
        artifact_id: "art_2".into(),
        mime: "application/pdf".into(),
        extracted_text: Some("content".into()),
    };

    assert!(dto.into_part_for_user_input().is_none());
}

#[test]
fn step_start_returns_none() {
    let dto = PartDto::StepStart { id: Uuid::nil() };

    assert!(dto.into_part_for_user_input().is_none());
}

#[test]
fn step_finish_returns_none() {
    let dto = PartDto::StepFinish {
        id: Uuid::nil(),
        reason: "end_turn".into(),
    };

    assert!(dto.into_part_for_user_input().is_none());
}

#[test]
fn compaction_returns_none() {
    let dto = PartDto::Compaction {
        id: Uuid::nil(),
        summary: "compacted".into(),
        compacted_message_ids: vec!["m1".into()],
        original_token_count: 5000,
    };

    assert!(dto.into_part_for_user_input().is_none());
}

#[test]
fn plan_returns_none() {
    let dto = PartDto::Plan {
        id: Uuid::nil(),
        plan: "step 1: do thing".into(),
    };

    assert!(dto.into_part_for_user_input().is_none());
}

//! Table-driven tests for `project_for_llm`.

use std::collections::HashMap;

use chrono::Utc;
use openlet_core::projection::{project_for_llm, LlmRole, ProjectionCaps};
use openlet_core::types::message::{Message, MessageId, Role};
use openlet_core::types::part::{Part, PartId};
use openlet_core::types::session::SessionId;

fn msg(role: Role) -> Message {
    Message {
        id: MessageId::new(),
        session_id: SessionId::new(),
        role,
        created_at: Utc::now(),
    }
}

#[test]
fn user_text_projects_to_user_message() {
    let m = msg(Role::User);
    let mut parts = HashMap::new();
    parts.insert(
        m.id,
        vec![Part::Text {
            id: PartId::new(),
            text: "hello".into(),
        }],
    );
    let out = project_for_llm(&[m], &parts, ProjectionCaps::default());
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].role, LlmRole::User);
    assert_eq!(out[0].content, "hello");
}

#[test]
fn assistant_tool_call_then_tool_result_pair_by_id() {
    let asst = msg(Role::Assistant);
    let tool = msg(Role::Tool);
    let mut parts = HashMap::new();
    parts.insert(
        asst.id,
        vec![Part::ToolCall {
            id: PartId::new(),
            call_id: "call-1".into(),
            name: "bash".into(),
            args: serde_json::json!({"cmd": "ls"}),
        }],
    );
    parts.insert(
        tool.id,
        vec![Part::ToolResult {
            id: PartId::new(),
            call_id: "call-1".into(),
            ok: true,
            text: Some("ok".into()),
            error: None,
        }],
    );

    let out = project_for_llm(&[asst, tool], &parts, ProjectionCaps::default());
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].role, LlmRole::Assistant);
    assert_eq!(out[0].tool_calls.len(), 1);
    assert_eq!(out[0].tool_calls[0].id, "call-1");
    assert_eq!(out[1].role, LlmRole::Tool);
    assert_eq!(out[1].tool_call_id.as_deref(), Some("call-1"));
}

#[test]
fn reasoning_dropped_when_caps_disable_replay() {
    let m = msg(Role::Assistant);
    let mut parts = HashMap::new();
    parts.insert(
        m.id,
        vec![
            Part::Reasoning {
                id: PartId::new(),
                text: "thinking".into(),
            },
            Part::Text {
                id: PartId::new(),
                text: "answer".into(),
            },
        ],
    );
    let out = project_for_llm(&[m], &parts, ProjectionCaps::default());
    assert_eq!(out.len(), 1);
    assert!(out[0].reasoning.is_none());
    assert_eq!(out[0].content, "answer");
}

#[test]
fn reasoning_kept_when_caps_enable_replay() {
    let m = msg(Role::Assistant);
    let mut parts = HashMap::new();
    parts.insert(
        m.id,
        vec![Part::Reasoning {
            id: PartId::new(),
            text: "thinking".into(),
        }],
    );
    let caps = ProjectionCaps {
        supports_reasoning_replay: true,
        ..Default::default()
    };
    let out = project_for_llm(&[m], &parts, caps);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].reasoning.as_deref(), Some("thinking"));
}

#[test]
fn empty_user_with_no_parts_skipped() {
    let m = msg(Role::User);
    let parts = HashMap::new();
    let out = project_for_llm(&[m], &parts, ProjectionCaps::default());
    assert!(out.is_empty());
}

#[test]
fn append_only_prefix_invariant() {
    let m = msg(Role::User);
    let pid = PartId::new();
    let mut parts = HashMap::new();
    parts.insert(
        m.id,
        vec![Part::Text {
            id: pid,
            text: "hi".into(),
        }],
    );
    let prefix = project_for_llm(std::slice::from_ref(&m), &parts, ProjectionCaps::default());

    let m2 = msg(Role::Assistant);
    parts.insert(
        m2.id,
        vec![Part::Text {
            id: PartId::new(),
            text: "ok".into(),
        }],
    );
    let after = project_for_llm(&[m.clone(), m2], &parts, ProjectionCaps::default());
    assert_eq!(prefix[0], after[0]);
}

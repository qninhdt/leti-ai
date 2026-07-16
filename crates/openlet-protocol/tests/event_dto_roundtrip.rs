//! Smoke tests for `From<AgentEvent>` → `EventDto`.
//!
//! Constructs a minimal instance of every `AgentEvent` variant, converts
//! to `EventDto`, and asserts no panic. For selected variants we also
//! verify specific wire-shape properties.

use chrono::Utc;
use uuid::Uuid;

use openlet_core::runtime::question_registry::QuestionId;
use openlet_core::types::event::{
    AgentEvent, AskOption, AttachmentKind, DeltaKind, NotificationLevel, Usage,
};
use openlet_core::types::message::MessageId;
use openlet_core::types::part::PartId;
use openlet_core::types::permission::{AskId, Decision, PermissionRequest};
use openlet_core::types::session::{SessionId, SessionStatus};
use openlet_protocol::EventDto;

fn sid() -> SessionId {
    SessionId::from(Uuid::nil())
}

fn mid() -> MessageId {
    MessageId(Uuid::nil())
}

fn pid() -> PartId {
    PartId(Uuid::nil())
}

#[test]
fn session_status_converts() {
    let ev = AgentEvent::SessionStatus {
        session_id: sid(),
        status: SessionStatus::Running,
        at: Utc::now(),
    };
    let _dto: EventDto = ev.into();
}

#[test]
fn message_created_converts() {
    let ev = AgentEvent::MessageCreated {
        session_id: sid(),
        message_id: mid(),
        at: Utc::now(),
    };
    let _dto: EventDto = ev.into();
}

#[test]
fn part_created_converts() {
    let ev = AgentEvent::PartCreated {
        session_id: sid(),
        message_id: mid(),
        part_id: pid(),
        at: Utc::now(),
    };
    let _dto: EventDto = ev.into();
}

#[test]
fn part_delta_converts() {
    let ev = AgentEvent::PartDelta {
        session_id: sid(),
        message_id: mid(),
        part_id: pid(),
        delta_kind: DeltaKind::Text,
        delta: "hello".into(),
    };
    let _dto: EventDto = ev.into();
}

#[test]
fn part_updated_converts() {
    let ev = AgentEvent::PartUpdated {
        session_id: sid(),
        message_id: mid(),
        part_id: pid(),
    };
    let _dto: EventDto = ev.into();
}

#[test]
fn step_finished_carries_cost_decimal_str() {
    let ev = AgentEvent::StepFinished {
        session_id: sid(),
        message_id: mid(),
        reason: "end_turn".into(),
        usage: Some(Usage {
            input_tokens: 10,
            output_tokens: 20,
            cached_input_tokens: 0,
            cache_write_tokens: 0,
            cache_creation_input_tokens: 0,
            reasoning_tokens: 0,
            cost_usd: None,
        }),
        cost_decimal_str: Some("0.0001".into()),
    };

    let dto: EventDto = ev.into();
    let json = serde_json::to_value(&dto).unwrap();
    assert_eq!(json["cost_decimal_str"], "0.0001");
    assert!(json["usage"].is_object());
}

#[test]
fn step_finished_without_usage() {
    let ev = AgentEvent::StepFinished {
        session_id: sid(),
        message_id: mid(),
        reason: "max_tokens".into(),
        usage: None,
        cost_decimal_str: None,
    };

    let dto: EventDto = ev.into();
    let json = serde_json::to_value(&dto).unwrap();
    // Optional fields are skipped when None.
    assert!(json.get("usage").is_none());
    assert!(json.get("cost_decimal_str").is_none());
}

#[test]
fn permission_asked_converts() {
    let ev = AgentEvent::PermissionAsked {
        session_id: sid(),
        ask_id: AskId(Uuid::nil()),
        request: PermissionRequest {
            permission: "bash:rm -rf /tmp/x".into(),
            reason: Some("cleanup".into()),
            timeout: None,
        },
    };
    let _dto: EventDto = ev.into();
}

#[test]
fn permission_resolved_allow_converts() {
    let ev = AgentEvent::PermissionResolved {
        session_id: sid(),
        ask_id: AskId(Uuid::nil()),
        decision: Decision::Allow,
    };

    let dto: EventDto = ev.into();
    let json = serde_json::to_value(&dto).unwrap();
    assert_eq!(json["decision"]["outcome"], "allow");
}

#[test]
fn permission_resolved_deny_carries_feedback() {
    let ev = AgentEvent::PermissionResolved {
        session_id: sid(),
        ask_id: AskId(Uuid::nil()),
        decision: Decision::Deny {
            feedback: Some("not allowed".into()),
        },
    };

    let dto: EventDto = ev.into();
    let json = serde_json::to_value(&dto).unwrap();
    assert_eq!(json["decision"]["outcome"], "deny");
    assert_eq!(json["decision"]["feedback"], "not allowed");
}

#[test]
fn permission_resolved_pending_variant() {
    // The Pending variant is technically unreachable in resolved-permission
    // events (it only exists as an intermediate state), but the conversion
    // handles it gracefully.
    let ev = AgentEvent::PermissionResolved {
        session_id: sid(),
        ask_id: AskId(Uuid::nil()),
        decision: Decision::Pending {
            ask_id: AskId(Uuid::nil()),
        },
    };

    let dto: EventDto = ev.into();
    let json = serde_json::to_value(&dto).unwrap();
    assert_eq!(json["decision"]["outcome"], "pending");
}

#[test]
fn error_converts() {
    let ev = AgentEvent::Error {
        session_id: Some(sid()),
        code: "internal".into(),
        message: "something broke".into(),
    };
    let _dto: EventDto = ev.into();
}

#[test]
fn error_without_session_converts() {
    let ev = AgentEvent::Error {
        session_id: None,
        code: "startup".into(),
        message: "config missing".into(),
    };
    let _dto: EventDto = ev.into();
}

#[test]
fn plugin_error_converts() {
    let ev = AgentEvent::PluginError {
        session_id: None,
        plugin_id: "my-plugin".into(),
        hook: "on_message".into(),
        message: "timeout".into(),
    };
    let _dto: EventDto = ev.into();
}

#[test]
fn question_requested_converts() {
    let ev = AgentEvent::QuestionRequested {
        session_id: sid(),
        question_id: QuestionId::from(Uuid::nil()),
        header: "Pick one".into(),
        question: "Which option?".into(),
        options: vec![
            AskOption {
                label: "A".into(),
                description: Some("option a".into()),
            },
            AskOption {
                label: "B".into(),
                description: None,
            },
        ],
        multi_select: false,
    };
    let _dto: EventDto = ev.into();
}

#[test]
fn plan_mode_entered_converts() {
    let ev = AgentEvent::PlanModeEntered {
        session_id: sid(),
        at: Utc::now(),
    };
    let _dto: EventDto = ev.into();
}

#[test]
fn plan_mode_exited_converts() {
    let ev = AgentEvent::PlanModeExited {
        session_id: sid(),
        plan: "1. do x\n2. do y".into(),
        at: Utc::now(),
    };
    let _dto: EventDto = ev.into();
}

#[test]
fn attachment_accepted_converts() {
    let ev = AgentEvent::AttachmentAccepted {
        session_id: sid(),
        message_id: mid(),
        part_id: pid(),
        artifact_id: "art_abc".into(),
        attachment_kind: AttachmentKind::Image,
        mime: "image/png".into(),
        summary: "PNG 800x600".into(),
    };
    let _dto: EventDto = ev.into();
}

#[test]
fn subagent_spawned_converts() {
    let ev = AgentEvent::SubagentSpawned {
        task_id: Uuid::nil(),
        tool_call_id: "call-1".into(),
        child_session_id: sid(),
        parent_session_id: sid(),
        subagent_type: "researcher".into(),
        objective: "research".into(),
        description: None,
        background: false,
    };
    let _dto: EventDto = ev.into();
}

#[test]
fn subagent_progress_converts() {
    let ev = AgentEvent::SubagentProgress {
        task_id: Uuid::nil(),
        parent_session_id: sid(),
        delta: "partial result".into(),
    };
    let _dto: EventDto = ev.into();
}

#[test]
fn subagent_settled_converts() {
    let ev = AgentEvent::SubagentSettled {
        task_id: Uuid::nil(),
        child_session_id: sid(),
        parent_session_id: sid(),
        status: "finished".into(),
        cost_usd: Some("0.05".into()),
    };
    let _dto: EventDto = ev.into();
}

#[test]
fn subagent_message_converts() {
    let ev = AgentEvent::SubagentMessage {
        task_id: Uuid::nil(),
        parent_session_id: sid(),
        from: "reviewer".into(),
        to: "worker#2".into(),
    };
    let _dto: EventDto = ev.into();
}

#[test]
fn subagent_roster_converts() {
    let ev = AgentEvent::SubagentRoster {
        root_session_id: sid(),
        entries: vec![openlet_core::types::event::RosterFrameEntry {
            name: "reviewer".into(),
            task_id: Uuid::nil(),
            generation: 7,
        }],
    };
    let _dto: EventDto = ev.into();
}

#[test]
fn notification_emitted_converts() {
    let ev = AgentEvent::NotificationEmitted {
        session_id: Some(sid()),
        level: NotificationLevel::Warn,
        title: "Rate limit".into(),
        body: "Approaching limit".into(),
        plugin_id: "rate-limiter".into(),
    };
    let _dto: EventDto = ev.into();
}

#[test]
fn todo_updated_converts() {
    let ev = AgentEvent::TodoUpdated {
        session_id: sid(),
        items: vec![
            openlet_core::types::event::TodoEventItem {
                content: "write tests".into(),
                status: "in_progress".into(),
                priority: "high".into(),
            },
            openlet_core::types::event::TodoEventItem {
                content: "ship it".into(),
                status: "pending".into(),
                priority: "low".into(),
            },
        ],
    };
    let dto: EventDto = ev.into();
    let json = serde_json::to_value(&dto).unwrap();
    // Serde tag is snake_case (`todo_updated`), distinct from the dotted
    // SSE event name `AgentEvent::kind()` returns (`todo.updated`).
    assert_eq!(json["kind"], "todo_updated");
    assert_eq!(json["items"][0]["status"], "in_progress");
    assert_eq!(json["items"][1]["priority"], "low");
}

#[test]
fn heartbeat_converts() {
    let ev = AgentEvent::Heartbeat;
    let _dto: EventDto = ev.into();
}

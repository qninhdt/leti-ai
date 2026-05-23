//! Deterministic projection: domain `Message`+`Part` -> LLM-shape messages.
//!
//! Centralized so phase-03's loop and phase-07's compaction share one rule
//! set. Pure function over a snapshot of messages and parts; appending a
//! part to the source never invalidates a prior projection's prefix.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::types::message::{Message, MessageId, Role};
use crate::types::part::Part;

/// Provider capability flags consulted while projecting. Phase-03 will fill
/// these from `ModelProvider::capabilities()`; today only thinking-back is
/// observed but the struct is here so adding more capabilities is additive.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProjectionCaps {
    pub supports_reasoning_replay: bool,
    pub supports_image_input: bool,
}

/// One LLM-shape message. `tool_calls` and `tool` role messages are paired
/// by `tool_call_id`. Content can be a plain string or a multi-part array
/// when the assistant emits images (forward-compat for phase-06).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LlmRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmToolCall {
    pub id: String,
    pub name: String,
    /// JSON-encoded arguments. Provider client serializes per its dialect.
    pub args_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmMessage {
    pub role: LlmRole,
    /// Plain content (assistant/user/system/tool result). Empty when only
    /// tool calls are present.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub content: String,
    /// Reasoning preamble. Emitted only when caps allow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    /// Assistant tool-call list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<LlmToolCall>,
    /// `tool` role only — paired with the assistant tool_call by id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

/// Project a session's message log into LLM-shape messages.
#[must_use]
pub fn project_for_llm(
    msgs: &[Message],
    parts_by_msg: &HashMap<MessageId, Vec<Part>>,
    caps: ProjectionCaps,
) -> Vec<LlmMessage> {
    let mut out = Vec::with_capacity(msgs.len());
    for msg in msgs {
        let parts = parts_by_msg
            .get(&msg.id)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        match msg.role {
            Role::System => project_system(parts, &mut out),
            Role::User => project_user(parts, &mut out),
            Role::Assistant => project_assistant(parts, caps, &mut out),
            Role::Tool => project_tool(parts, &mut out),
        }
    }
    out
}

fn collect_text(parts: &[Part]) -> String {
    let mut buf = String::new();
    for p in parts {
        if let Part::Text { text, .. } = p {
            if !buf.is_empty() {
                buf.push('\n');
            }
            buf.push_str(text);
        }
    }
    buf
}

fn project_system(parts: &[Part], out: &mut Vec<LlmMessage>) {
    let content = collect_text(parts);
    if content.is_empty() {
        return;
    }
    out.push(LlmMessage {
        role: LlmRole::System,
        content,
        reasoning: None,
        tool_calls: Vec::new(),
        tool_call_id: None,
    });
}

fn project_user(parts: &[Part], out: &mut Vec<LlmMessage>) {
    let content = collect_text(parts);
    if content.is_empty() && !parts.iter().any(|p| matches!(p, Part::Image { .. })) {
        return;
    }
    out.push(LlmMessage {
        role: LlmRole::User,
        content,
        reasoning: None,
        tool_calls: Vec::new(),
        tool_call_id: None,
    });
}

fn project_assistant(
    parts: &[Part],
    caps: ProjectionCaps,
    out: &mut Vec<LlmMessage>,
) {
    let mut content = String::new();
    let mut reasoning = String::new();
    let mut tool_calls: Vec<LlmToolCall> = Vec::new();

    for p in parts {
        match p {
            Part::Text { text, .. } => {
                if !content.is_empty() {
                    content.push('\n');
                }
                content.push_str(text);
            }
            Part::Reasoning { text, .. } if caps.supports_reasoning_replay => {
                if !reasoning.is_empty() {
                    reasoning.push('\n');
                }
                reasoning.push_str(text);
            }
            Part::ToolCall { call_id, name, args, .. } => {
                tool_calls.push(LlmToolCall {
                    id: call_id.clone(),
                    name: name.clone(),
                    args_json: args.to_string(),
                });
            }
            _ => {}
        }
    }

    if content.is_empty() && reasoning.is_empty() && tool_calls.is_empty() {
        return;
    }

    out.push(LlmMessage {
        role: LlmRole::Assistant,
        content,
        reasoning: if reasoning.is_empty() { None } else { Some(reasoning) },
        tool_calls,
        tool_call_id: None,
    });
}

fn project_tool(parts: &[Part], out: &mut Vec<LlmMessage>) {
    for p in parts {
        if let Part::ToolResult { call_id, ok, text, error, .. } = p {
            let body = if *ok {
                text.clone().unwrap_or_default()
            } else {
                error.clone().unwrap_or_else(|| "tool error".to_string())
            };
            out.push(LlmMessage {
                role: LlmRole::Tool,
                content: body,
                reasoning: None,
                tool_calls: Vec::new(),
                tool_call_id: Some(call_id.clone()),
            });
        }
    }
}

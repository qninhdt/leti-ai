//! Deterministic projection: domain `Message`+`Part` -> LLM-shape messages.
//!
//! Centralized so the turn loop and compaction share one rule
//! set. Pure function over a snapshot of messages and parts; appending a
//! part to the source never invalidates a prior projection's prefix.

use std::collections::{HashMap, HashSet};

pub mod types;
pub use types::{LlmMessage, LlmRole, LlmToolCall, ProjectionCaps};

use crate::types::message::{Message, MessageId, Role};
use crate::types::part::Part;

/// Project a session's message log into LLM-shape messages.
#[must_use]
pub fn project_for_llm(
    msgs: &[Message],
    parts_by_msg: &HashMap<MessageId, Vec<Part>>,
    caps: ProjectionCaps,
) -> Vec<LlmMessage> {
    let (compacted_ids, summaries) = collect_compactions(msgs, parts_by_msg);
    let mut out = Vec::with_capacity(msgs.len());
    let mut emitted_summary_for: HashSet<MessageId> = HashSet::new();
    for msg in msgs {
        // If this message was superseded by a compaction summary, emit the
        // summary in its place — exactly once per summary, on the first
        // compacted message in chronological order.
        if let Some(owner) = compacted_ids.get(&msg.id) {
            if emitted_summary_for.insert(*owner)
                && let Some(summary) = summaries.get(owner)
            {
                out.push(LlmMessage::simple(
                    LlmRole::System,
                    format!("[Compacted conversation summary]\n{summary}"),
                ));
            }
            continue;
        }
        let parts = parts_by_msg.get(&msg.id).map(Vec::as_slice).unwrap_or(&[]);
        match msg.role {
            Role::System => project_system(parts, &mut out),
            Role::User => project_user(parts, caps, &mut out),
            Role::Assistant => project_assistant(parts, caps, &mut out),
            Role::Tool => project_tool(parts, &mut out),
        }
    }
    out
}

/// Walk parts; for each `Part::Compaction`, map every superseded message id
/// to the compaction's owning message id, and store the summary text by
/// owner. Returns `(superseded_id -> owner_id, owner_id -> summary)`.
fn collect_compactions(
    msgs: &[Message],
    parts_by_msg: &HashMap<MessageId, Vec<Part>>,
) -> (HashMap<MessageId, MessageId>, HashMap<MessageId, String>) {
    use uuid::Uuid;
    let mut superseded: HashMap<MessageId, MessageId> = HashMap::new();
    let mut summaries: HashMap<MessageId, String> = HashMap::new();
    for m in msgs {
        let Some(parts) = parts_by_msg.get(&m.id) else {
            continue;
        };
        for p in parts {
            if let Part::Compaction {
                summary,
                compacted_message_ids,
                ..
            } = p
            {
                summaries.insert(m.id, summary.clone());
                for raw in compacted_message_ids {
                    if let Ok(uuid) = Uuid::parse_str(raw) {
                        superseded.insert(MessageId(uuid), m.id);
                    }
                }
            }
        }
    }
    (superseded, summaries)
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
    out.push(LlmMessage::simple(LlmRole::System, content));
}

fn project_user(parts: &[Part], caps: ProjectionCaps, out: &mut Vec<LlmMessage>) {
    let mut content = collect_text(parts);
    let attachment_text = collect_attachment_fallback_text(parts, caps);
    if !attachment_text.is_empty() {
        if !content.is_empty() {
            content.push('\n');
        }
        content.push_str(&attachment_text);
    }
    let has_image_part = parts.iter().any(|p| matches!(p, Part::Image { .. }));
    if content.is_empty() && !has_image_part {
        return;
    }
    out.push(LlmMessage::simple(LlmRole::User, content));
}

/// Gather a text fallback for `Part::Image` / `Part::Document` parts.
///
/// Projection emits image / document attachments as inline
/// text placeholders (with extracted PDF text inlined when available).
/// A future revision will fork on `ProjectionCaps` to emit native multipart
/// content for vision-capable providers; today both code paths share
/// the text fallback so the model still sees *something* about the
/// attachment.
fn collect_attachment_fallback_text(parts: &[Part], caps: ProjectionCaps) -> String {
    let mut buf = String::new();
    for p in parts {
        match p {
            Part::Image {
                artifact_id, mime, ..
            } => {
                if caps.supports_image_input {
                    // Vision-capable models receive the image as a
                    // multipart block at the wire layer; the text
                    // fallback is suppressed so the placeholder text
                    // doesn't compete with the actual image content.
                    continue;
                }
                if !buf.is_empty() {
                    buf.push('\n');
                }
                buf.push_str(&format!(
                    "[image artifact {artifact_id} ({mime}) — vision not supported on this model]"
                ));
            }
            Part::Document {
                artifact_id,
                mime,
                extracted_text,
                ..
            } => {
                if !buf.is_empty() {
                    buf.push('\n');
                }
                buf.push_str(&format!("[document artifact {artifact_id} ({mime})]"));
                // Inline the extracted text only when the provider can't
                // ingest the document natively — otherwise the wire layer
                // attaches the original bytes and the text would be noise.
                if !caps.supports_document_input
                    && let Some(text) = extracted_text
                {
                    buf.push('\n');
                    buf.push_str(text);
                }
            }
            _ => {}
        }
    }
    buf
}

fn project_assistant(parts: &[Part], caps: ProjectionCaps, out: &mut Vec<LlmMessage>) {
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
            Part::ToolCall {
                call_id,
                name,
                args,
                ..
            } => {
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
        reasoning: if reasoning.is_empty() {
            None
        } else {
            Some(reasoning)
        },
        tool_calls,
        tool_call_id: None,
    });
}

fn project_tool(parts: &[Part], out: &mut Vec<LlmMessage>) {
    for p in parts {
        if let Part::ToolResult {
            call_id,
            ok,
            text,
            error,
            ..
        } = p
        {
            let body = if *ok {
                text.clone().unwrap_or_default()
            } else {
                error.clone().unwrap_or_else(|| "tool error".to_string())
            };
            out.push(LlmMessage::tool(call_id.clone(), body));
        }
        // Part::Plan is durably persisted alongside the tool message
        // for audit/replay, but the model already sees the plan via
        // the matching ToolResult. Skipping here keeps projection
        // deterministic and prevents double-attribution.
    }
}

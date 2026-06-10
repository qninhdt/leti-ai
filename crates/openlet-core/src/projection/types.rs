//! LLM-shape message types — extracted from `projection.rs` so the
//! type definitions live separately from the projection algorithm.
//!
//! `ProjectionCaps` flags which provider features the projection layer
//! should honor. `LlmMessage` / `LlmToolCall` / `LlmRole` are the
//! provider-facing wire shape; the OpenAI-compat adapter serializes
//! them per-dialect.

use serde::{Deserialize, Serialize};

/// Provider capability flags consulted while projecting. Phase-03 will
/// fill these from `ModelProvider::capabilities()`; today only
/// thinking-back is observed but the struct is here so adding more
/// capabilities is additive.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProjectionCaps {
    pub supports_reasoning_replay: bool,
    pub supports_image_input: bool,
    pub supports_document_input: bool,
}

/// One LLM-shape message. `tool_calls` and `tool` role messages are
/// paired by `tool_call_id`. Content can be a plain string or a
/// multi-part array when the assistant emits images (forward-compat).
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
    /// Plain content (assistant/user/system/tool result). Empty when
    /// only tool calls are present.
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

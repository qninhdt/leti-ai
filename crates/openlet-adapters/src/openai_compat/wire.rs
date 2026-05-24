//! OpenAI-compat chat-completions wire types.
//!
//! Concrete JSON shape for `POST /v1/chat/completions` request + the
//! streamed `chat.completion.chunk` response. Decoupled from the domain
//! `ChatRequest` / `ChatDelta` so the provider can absorb provider-specific
//! quirks (OpenRouter's `provider` block, gateway-only fields) without
//! polluting `openlet-core`.

use serde::Serialize;

use openlet_core::adapters::model_provider::{ChatRequest, ToolSpec};
use openlet_core::projection::{LlmMessage as ProjMsg, LlmRole, LlmToolCall};

/// Outbound `POST /v1/chat/completions` request body.
#[derive(Debug, Clone, Serialize)]
pub struct OpenAiRequest<'a> {
    pub model: &'a str,
    pub messages: Vec<OpenAiMessage<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<OpenAiTool<'a>>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<StreamOptions>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StreamOptions {
    pub include_usage: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct OpenAiMessage<'a> {
    pub role: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<OpenAiToolCall<'a>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OpenAiToolCall<'a> {
    pub id: &'a str,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: OpenAiToolFn<'a>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OpenAiToolFn<'a> {
    pub name: &'a str,
    pub arguments: &'a str,
}

#[derive(Debug, Clone, Serialize)]
pub struct OpenAiTool<'a> {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: OpenAiToolDef<'a>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OpenAiToolDef<'a> {
    pub name: &'a str,
    pub description: &'a str,
    pub parameters: &'a serde_json::Value,
}

/// Build an `OpenAiRequest` from the domain `ChatRequest`. System messages
/// from projection are serialized as `role: "system"` rows; OpenAI does not
/// accept the Anthropic-style top-level `system` field, so the dedicated
/// `system` slot in `ChatRequest` is prepended as a system message here.
#[must_use]
pub fn to_wire<'a>(req: &'a ChatRequest) -> OpenAiRequest<'a> {
    let mut messages = Vec::with_capacity(req.messages.len() + 1);
    if let Some(sys) = req.system.as_deref() {
        if !sys.is_empty() {
            messages.push(OpenAiMessage {
                role: "system",
                content: Some(sys.to_string()),
                tool_call_id: None,
                tool_calls: Vec::new(),
            });
        }
    }
    for m in &req.messages {
        messages.push(project_msg(m));
    }

    let tools = req.tools.iter().map(tool_to_wire).collect();

    OpenAiRequest {
        model: req.model.as_str(),
        messages,
        max_tokens: req.max_tokens,
        temperature: req.temperature,
        tools,
        stream: req.stream,
        stream_options: req.stream.then_some(StreamOptions {
            include_usage: true,
        }),
    }
}

fn project_msg(m: &ProjMsg) -> OpenAiMessage<'_> {
    let role = match m.role {
        LlmRole::System => "system",
        LlmRole::User => "user",
        LlmRole::Assistant => "assistant",
        LlmRole::Tool => "tool",
    };
    let content = if m.content.is_empty() && !matches!(m.role, LlmRole::Assistant) {
        None
    } else {
        Some(m.content.clone())
    };
    OpenAiMessage {
        role,
        content,
        tool_call_id: m.tool_call_id.as_deref(),
        tool_calls: m
            .tool_calls
            .iter()
            .map(|c: &LlmToolCall| OpenAiToolCall {
                id: c.id.as_str(),
                kind: "function",
                function: OpenAiToolFn {
                    name: c.name.as_str(),
                    arguments: c.args_json.as_str(),
                },
            })
            .collect(),
    }
}

fn tool_to_wire(t: &ToolSpec) -> OpenAiTool<'_> {
    OpenAiTool {
        kind: "function",
        function: OpenAiToolDef {
            name: t.name.as_str(),
            description: t.description.as_str(),
            parameters: &t.parameters,
        },
    }
}

// Suppress unused-import lint when only `to_wire` is exercised.
#[allow(dead_code)]
fn _project_msg_alias(m: &ProjMsg) -> &str {
    match m.role {
        LlmRole::System => "system",
        LlmRole::User => "user",
        LlmRole::Assistant => "assistant",
        LlmRole::Tool => "tool",
    }
}

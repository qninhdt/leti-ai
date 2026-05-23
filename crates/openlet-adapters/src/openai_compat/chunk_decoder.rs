//! Decodes one OpenAI-compat `chat.completion.chunk` JSON object into zero or
//! more `ChatDelta` values. Pure logic, no IO.
//!
//! OpenAI streams a sequence of these chunks; the relevant fields are
//! `choices[0].delta.{role,content,reasoning_content,tool_calls}` plus the
//! per-chunk `choices[0].finish_reason` and the trailing `usage` block when
//! `stream_options.include_usage = true`.

use serde::Deserialize;

use openlet_core::adapters::model_provider::{ChatDelta, FinishReason};
use openlet_core::error::ProviderError;
use openlet_core::types::event::Usage;

#[derive(Debug, Deserialize)]
pub struct ChunkEnvelope {
    #[serde(default)]
    pub choices: Vec<ChunkChoice>,
    #[serde(default)]
    pub usage: Option<UsageWire>,
}

#[derive(Debug, Deserialize)]
pub struct ChunkChoice {
    #[serde(default)]
    pub delta: ChunkDelta,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ChunkDelta {
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    /// OpenAI o1/o3 + DeepSeek surface reasoning text on this field.
    #[serde(default)]
    pub reasoning_content: Option<String>,
    /// OpenRouter alias for the same — some upstreams forward as `reasoning`.
    #[serde(default)]
    pub reasoning: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<ChunkToolCall>,
}

#[derive(Debug, Deserialize)]
pub struct ChunkToolCall {
    pub index: usize,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub function: Option<ChunkFn>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ChunkFn {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UsageWire {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub prompt_tokens_details: Option<PromptTokenDetails>,
}

#[derive(Debug, Default, Deserialize)]
pub struct PromptTokenDetails {
    #[serde(default)]
    pub cached_tokens: u64,
}

impl UsageWire {
    fn into_usage(self) -> Usage {
        let cached = self
            .prompt_tokens_details
            .map(|d| d.cached_tokens)
            .unwrap_or(0);
        Usage {
            input_tokens: self.prompt_tokens,
            output_tokens: self.completion_tokens,
            cached_input_tokens: cached,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
        }
    }
}

/// Decode one chunk JSON payload into deltas. The trailing chunk (with
/// `usage` populated and choices empty) emits a `Finish` carrying usage.
pub fn decode_chunk(payload: &str) -> Result<Vec<ChatDelta>, ProviderError> {
    let env: ChunkEnvelope = serde_json::from_str(payload)
        .map_err(|e| ProviderError::Decode(format!("chunk envelope: {e}")))?;

    let mut out: Vec<ChatDelta> = Vec::new();
    let choices_empty = env.choices.is_empty();

    for choice in env.choices {
        let ChunkChoice {
            delta,
            finish_reason,
        } = choice;

        if delta.role.is_some() {
            out.push(ChatDelta::Role);
        }

        if let Some(text) = delta.content {
            if !text.is_empty() {
                out.push(ChatDelta::Content { text });
            }
        }

        let reasoning = delta.reasoning_content.or(delta.reasoning);
        if let Some(text) = reasoning {
            if !text.is_empty() {
                out.push(ChatDelta::Reasoning {
                    text,
                    signature: None,
                });
            }
        }

        for tc in delta.tool_calls {
            push_tool_delta(tc, &mut out);
        }

        if let Some(reason) = finish_reason {
            let mapped = map_finish_reason(&reason);
            // Usage may arrive on the same chunk as finish (some gateways) or
            // on the dedicated trailing usage chunk. We attach it if present
            // here; otherwise the trailing usage chunk emits its own Finish.
            let usage = env.usage.as_ref().map(|u| Usage {
                input_tokens: u.prompt_tokens,
                output_tokens: u.completion_tokens,
                cached_input_tokens: u
                    .prompt_tokens_details
                    .as_ref()
                    .map(|d| d.cached_tokens)
                    .unwrap_or(0),
                cache_write_tokens: 0,
                reasoning_tokens: 0,
            });
            out.push(ChatDelta::Finish {
                reason: mapped,
                usage,
            });
            return Ok(out);
        }
    }

    // Trailing usage-only chunk.
    if choices_empty {
        if let Some(u) = env.usage {
            out.push(ChatDelta::Finish {
                reason: FinishReason::EndTurn,
                usage: Some(u.into_usage()),
            });
        }
    }

    Ok(out)
}

fn push_tool_delta(tc: ChunkToolCall, out: &mut Vec<ChatDelta>) {
    let ChunkToolCall { index, id, function } = tc;
    let (name, arguments) = match function {
        Some(f) => (f.name, f.arguments),
        None => (None, None),
    };

    // First chunk for this index carries id + name; OpenAI guarantees they
    // appear together on the opening delta.
    if let (Some(call_id), Some(fn_name)) = (id.as_ref(), name.as_ref()) {
        out.push(ChatDelta::ToolCallStart {
            call_id: call_id.clone(),
            name: fn_name.clone(),
            index,
        });
    } else if let Some(call_id) = id {
        // Fallback: id without name (rare). Use empty name; processor will
        // surface a decode error if name never arrives.
        out.push(ChatDelta::ToolCallStart {
            call_id,
            name: String::new(),
            index,
        });
    }

    if let Some(chunk) = arguments {
        if !chunk.is_empty() {
            out.push(ChatDelta::ToolCallArgsDelta {
                index,
                args_chunk: chunk,
            });
        }
    }
}

fn map_finish_reason(s: &str) -> FinishReason {
    match s {
        "stop" => FinishReason::EndTurn,
        "tool_calls" | "function_call" => FinishReason::ToolUse,
        "length" => FinishReason::MaxTokens,
        "content_filter" => FinishReason::ContentFilter,
        _ => FinishReason::Error,
    }
}

#[cfg(test)]
mod tests {
    use super::{decode_chunk, FinishReason};
    use openlet_core::adapters::model_provider::ChatDelta;

    #[test]
    fn decodes_role_then_text() {
        let p = r#"{"choices":[{"delta":{"role":"assistant","content":"Hi"}}]}"#;
        let deltas = decode_chunk(p).unwrap();
        assert!(matches!(deltas[0], ChatDelta::Role));
        assert!(matches!(&deltas[1], ChatDelta::Content { text } if text == "Hi"));
    }

    #[test]
    fn decodes_tool_call_open_then_args() {
        let p = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"bash","arguments":"{\"cmd\""}}]}}]}"#;
        let deltas = decode_chunk(p).unwrap();
        assert!(matches!(
            &deltas[0],
            ChatDelta::ToolCallStart { call_id, name, index } if call_id == "call_1" && name == "bash" && *index == 0
        ));
        assert!(matches!(
            &deltas[1],
            ChatDelta::ToolCallArgsDelta { index, args_chunk } if *index == 0 && args_chunk == "{\"cmd\""
        ));
    }

    #[test]
    fn finish_with_inline_usage() {
        let p = r#"{"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5}}"#;
        let deltas = decode_chunk(p).unwrap();
        assert!(matches!(
            &deltas[0],
            ChatDelta::Finish { reason: FinishReason::EndTurn, usage: Some(_) }
        ));
    }

    #[test]
    fn trailing_usage_chunk_emits_finish_endturn() {
        let p = r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5}}"#;
        let deltas = decode_chunk(p).unwrap();
        assert!(matches!(
            &deltas[0],
            ChatDelta::Finish { reason: FinishReason::EndTurn, usage: Some(u) } if u.input_tokens == 10
        ));
    }

    #[test]
    fn maps_tool_calls_finish_reason() {
        let p = r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#;
        let deltas = decode_chunk(p).unwrap();
        assert!(matches!(
            &deltas[0],
            ChatDelta::Finish { reason: FinishReason::ToolUse, .. }
        ));
    }

    #[test]
    fn surfaces_reasoning_content() {
        let p = r#"{"choices":[{"delta":{"reasoning_content":"think"}}]}"#;
        let deltas = decode_chunk(p).unwrap();
        assert!(matches!(
            &deltas[0],
            ChatDelta::Reasoning { text, .. } if text == "think"
        ));
    }
}

//! Decodes one OpenAI-compat `chat.completion.chunk` JSON object into zero or
//! more `ChatDelta` values. Pure logic, no IO.
//!
//! OpenAI streams a sequence of these chunks; the relevant fields are
//! `choices[0].delta.{role,content,reasoning_content,tool_calls}` plus the
//! per-chunk `choices[0].finish_reason` and the trailing `usage` block when
//! `stream_options.include_usage = true`.

use serde::Deserialize;
use serde::de::Deserializer;

use openlet_core::adapters::model_provider::{ChatDelta, FinishReason};
use openlet_core::error::ProviderError;
use openlet_core::types::event::Usage;

/// Deserialize a value that may be `null` into `T::default()`.
///
/// `#[serde(default)]` only fills in a value when the field is ABSENT; an
/// explicit `null` still routes through `T`'s `Deserialize`, which fails for
/// `Vec<_>` with "invalid type: null, expected a sequence". Some
/// OpenAI-compat gateways (e.g. Gemini-via-proxy) send `"tool_calls": null`
/// and `"choices": null` on chunks with no tool call — this coerces those to
/// an empty collection instead of aborting the whole stream.
fn null_to_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Debug, Deserialize)]
pub struct ChunkEnvelope {
    #[serde(default, deserialize_with = "null_to_default")]
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
    #[serde(default, deserialize_with = "null_to_default")]
    pub tool_calls: Vec<ChunkToolCall>,
}

#[derive(Debug, Deserialize)]
pub struct ChunkToolCall {
    /// Position of this tool call within the turn. OpenAI/OpenRouter always
    /// send it (it's how streamed arg deltas are grouped), but some
    /// gateways — notably Gemini-via-proxy — omit it on tool-call deltas.
    /// Optional here so the whole stream doesn't abort with "missing field
    /// `index`"; the decoder falls back to the tool call's array position.
    #[serde(default)]
    pub index: Option<usize>,
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
    /// OpenRouter's authoritative turn cost in USD, returned in the final
    /// usage chunk when `stream_options.include_usage` is set. Carried
    /// through to the domain `Usage` so the displayed cost reflects real
    /// billing for every model with no static pricing row.
    #[serde(default)]
    pub cost: Option<rust_decimal::Decimal>,
}

#[derive(Debug, Default, Deserialize)]
pub struct PromptTokenDetails {
    #[serde(default)]
    pub cached_tokens: u64,
}

impl UsageWire {
    /// Borrowing conversion to the domain `Usage`. Used on the trailing
    /// usage-only chunk (consuming, via [`into_usage`]) and on the
    /// finish-with-inline-usage path (by-ref, since `env.usage` must stay
    /// available for later choices in the same envelope).
    fn to_usage(&self) -> Usage {
        let cached = self
            .prompt_tokens_details
            .as_ref()
            .map(|d| d.cached_tokens)
            .unwrap_or(0);
        Usage {
            input_tokens: self.prompt_tokens,
            output_tokens: self.completion_tokens,
            cached_input_tokens: cached,
            cost_usd: self.cost,
            ..Default::default()
        }
    }

    fn into_usage(self) -> Usage {
        self.to_usage()
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

        for (pos, tc) in delta.tool_calls.into_iter().enumerate() {
            push_tool_delta(tc, pos, &mut out);
        }

        if let Some(reason) = finish_reason {
            let mapped = map_finish_reason(&reason);
            // Usage may arrive on the same chunk as finish (some gateways) or
            // on the dedicated trailing usage chunk. We attach it if present
            // here; otherwise the trailing usage chunk emits its own Finish.
            let usage = env.usage.as_ref().map(UsageWire::to_usage);
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

fn push_tool_delta(tc: ChunkToolCall, pos: usize, out: &mut Vec<ChatDelta>) {
    let ChunkToolCall {
        index,
        id,
        function,
    } = tc;
    // Gateways that omit `index` (Gemini-via-proxy) get the array position as
    // a stable fallback: within one chunk the ordering is authoritative, and
    // single-tool-call turns (the common case) always collapse to index 0.
    let index = index.unwrap_or(pos);
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
    use super::{FinishReason, decode_chunk};
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
            ChatDelta::Finish {
                reason: FinishReason::EndTurn,
                usage: Some(_)
            }
        ));
    }

    #[test]
    fn parses_openrouter_cost_into_usage() {
        // OpenRouter returns an authoritative turn cost on the trailing
        // usage chunk. It must reach `Usage.cost_usd` so the cost path
        // prefers it over the static pricing table.
        let p =
            r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5,"cost":0.0123}}"#;
        let deltas = decode_chunk(p).unwrap();
        match &deltas[0] {
            ChatDelta::Finish { usage: Some(u), .. } => assert_eq!(
                u.cost_usd,
                Some(rust_decimal::Decimal::from_str_exact("0.0123").unwrap())
            ),
            other => panic!("expected Finish with usage, got {other:?}"),
        }
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
            ChatDelta::Finish {
                reason: FinishReason::ToolUse,
                ..
            }
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

    #[test]
    fn tolerates_null_tool_calls() {
        // Gemini-via-gateway sends `"tool_calls": null` on text-only chunks.
        // `#[serde(default)]` alone rejects explicit null ("invalid type:
        // null, expected a sequence") — the whole turn used to crash here.
        let p = r#"{"choices":[{"delta":{"role":"assistant","content":"hi","tool_calls":null}}]}"#;
        let deltas = decode_chunk(p).unwrap();
        assert!(matches!(deltas[0], ChatDelta::Role));
        assert!(matches!(&deltas[1], ChatDelta::Content { text } if text == "hi"));
        // No tool-call deltas emitted for a null list.
        assert_eq!(deltas.len(), 2);
    }

    #[test]
    fn tolerates_null_choices() {
        // Same class of bug on the envelope's `choices` array.
        let p = r#"{"choices":null,"usage":{"prompt_tokens":3,"completion_tokens":1}}"#;
        let deltas = decode_chunk(p).unwrap();
        // choices null → treated as empty → trailing usage still emits Finish.
        assert!(matches!(
            &deltas[0],
            ChatDelta::Finish { usage: Some(u), .. } if u.input_tokens == 3
        ));
    }

    #[test]
    fn tolerates_null_choices_no_usage() {
        // Null choices with no usage → no deltas, no error (stream continues).
        let p = r#"{"choices":null}"#;
        let deltas = decode_chunk(p).unwrap();
        assert!(deltas.is_empty());
    }

    #[test]
    fn tool_call_without_index_falls_back_to_array_position() {
        // Gemini-via-gateway omits `index` on tool-call deltas. A required
        // `index` field aborted the whole turn with "missing field `index`";
        // the decoder must fall back to the tool call's position instead.
        let p = r#"{"choices":[{"delta":{"tool_calls":[{"id":"call_1","function":{"name":"todo","arguments":"{"}}]}}]}"#;
        let deltas = decode_chunk(p).unwrap();
        assert!(matches!(
            &deltas[0],
            ChatDelta::ToolCallStart { call_id, name, index }
                if call_id == "call_1" && name == "todo" && *index == 0
        ));
        assert!(matches!(
            &deltas[1],
            ChatDelta::ToolCallArgsDelta { index, .. } if *index == 0
        ));
    }

    #[test]
    fn two_indexless_tool_calls_get_distinct_positions() {
        // Parallel tool calls in one chunk with no `index` must not collide —
        // array position keeps them on separate BTreeMap keys downstream.
        let p = r#"{"choices":[{"delta":{"tool_calls":[
            {"id":"a","function":{"name":"read","arguments":"{}"}},
            {"id":"b","function":{"name":"glob","arguments":"{}"}}
        ]}}]}"#;
        let deltas = decode_chunk(p).unwrap();
        let starts: Vec<usize> = deltas
            .iter()
            .filter_map(|d| match d {
                ChatDelta::ToolCallStart { index, .. } => Some(*index),
                _ => None,
            })
            .collect();
        assert_eq!(starts, vec![0, 1]);
    }
}

//! Scenario catalog for the mock OpenAI-compat service.
//!
//! Each variant defines either an SSE byte stream (200 OK) or a
//! JSON error body (>=400). Scenarios are selected by scanning the
//! inbound `messages[].content` for `PARITY_SCENARIO:<name>`.

use std::time::Duration;

/// Magic-token prefix embedded in user-message text to select a scenario.
/// Pattern ported verbatim from claw-code's `mock-anthropic-service`.
pub const SCENARIO_PREFIX: &str = "PARITY_SCENARIO:";

/// Closed enum of canned scenarios. Adding one means writing the bytes
/// in [`Scenario::render`] — no fixture files; everything is in-source
/// so the test crate has no IO surprises in CI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scenario {
    /// Streamed assistant turn: role + two content chunks + finish + usage.
    SimpleText,
    /// Streamed turn with one tool call (bash) — id+name on the opening
    /// chunk, args fragmented across two `partial_json` deltas.
    WithToolCall,
    /// Reasoning-content stream (o1/o3 / DeepSeek style).
    Reasoning,
    /// 413 Payload Too Large — context window exceeded.
    ContextOverflow,
    /// 429 Too Many Requests with `retry-after`.
    RateLimit,
    /// Mid-stream truncation: server closes after one chunk, no `[DONE]`.
    MidStreamCancel,
}

impl Scenario {
    /// Parse the scenario suffix (case-insensitive). Unknown names
    /// fall back to `SimpleText` so a stray token never 500s the test.
    #[must_use]
    pub fn parse(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "simple_text" => Self::SimpleText,
            "with_tool_call" => Self::WithToolCall,
            "reasoning" => Self::Reasoning,
            "context_overflow" => Self::ContextOverflow,
            "rate_limit" => Self::RateLimit,
            "mid_stream_cancel" => Self::MidStreamCancel,
            _ => Self::SimpleText,
        }
    }

    /// Render the full HTTP response (status line + headers + body).
    /// SSE scenarios stream the body; the `(bytes, inter_chunk_delay)`
    /// shape lets callers exercise chunked write + heartbeat behavior.
    #[must_use]
    pub fn render(self) -> Response {
        match self {
            Self::SimpleText => Response::sse(simple_text_frames()),
            Self::WithToolCall => Response::sse(with_tool_call_frames()),
            Self::Reasoning => Response::sse(reasoning_frames()),
            Self::ContextOverflow => Response::error(
                413,
                "Payload Too Large",
                r#"{"error":{"message":"context window exceeded","type":"context_length_exceeded","code":"context_length_exceeded"}}"#,
                &[],
            ),
            Self::RateLimit => Response::error(
                429,
                "Too Many Requests",
                r#"{"error":{"message":"rate limit exceeded","type":"rate_limit_exceeded","code":"rate_limited"}}"#,
                &[("retry-after", "1")],
            ),
            Self::MidStreamCancel => Response::sse(mid_stream_cancel_frames()),
        }
    }
}

/// Locate the `PARITY_SCENARIO:<name>` token inside a user message's text
/// content. Walks `messages[]` in reverse (claw-code parity) so the most
/// recent user turn wins. Returns `None` if no token is present.
#[must_use]
pub fn detect_scenario(messages: &serde_json::Value) -> Option<Scenario> {
    let arr = messages.as_array()?;
    for msg in arr.iter().rev() {
        // Skip messages without a `content` field (e.g. assistant turns
        // carrying only `tool_calls`) rather than aborting the whole
        // scan — earlier messages may still hold the token.
        let Some(content) = msg.get("content") else {
            continue;
        };
        if let Some(text) = content.as_str() {
            if let Some(s) = scan_token(text) {
                return Some(s);
            }
        } else if let Some(parts) = content.as_array() {
            for part in parts {
                if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                    if let Some(s) = scan_token(text) {
                        return Some(s);
                    }
                }
            }
        }
    }
    None
}

fn scan_token(text: &str) -> Option<Scenario> {
    for tok in text.split_whitespace() {
        if let Some(rest) = tok.strip_prefix(SCENARIO_PREFIX) {
            return Some(Scenario::parse(rest));
        }
    }
    None
}

/// Outbound HTTP response. SSE bodies are split into chunks so the
/// server task can `write_all` each, optionally yielding between
/// frames to exercise the parser's incremental-feed code path.
pub enum Response {
    Sse {
        chunks: Vec<Vec<u8>>,
        inter_chunk_delay: Duration,
    },
    Error {
        status: u16,
        status_text: &'static str,
        body: String,
        extra_headers: Vec<(&'static str, String)>,
    },
}

impl Response {
    fn sse(chunks: Vec<Vec<u8>>) -> Self {
        Self::Sse {
            chunks,
            inter_chunk_delay: Duration::from_millis(0),
        }
    }
    fn error(
        status: u16,
        status_text: &'static str,
        body: &str,
        extra_headers: &[(&'static str, &str)],
    ) -> Self {
        Self::Error {
            status,
            status_text,
            body: body.to_string(),
            extra_headers: extra_headers
                .iter()
                .map(|(k, v)| (*k, v.to_string()))
                .collect(),
        }
    }
}

fn frame(event: &str, payload: &str) -> Vec<u8> {
    if event.is_empty() {
        format!("data: {payload}\n\n").into_bytes()
    } else {
        format!("event: {event}\ndata: {payload}\n\n").into_bytes()
    }
}

fn done_frame() -> Vec<u8> {
    b"data: [DONE]\n\n".to_vec()
}

fn simple_text_frames() -> Vec<Vec<u8>> {
    vec![
        frame("", r#"{"choices":[{"delta":{"role":"assistant"}}]}"#),
        frame("", r#"{"choices":[{"delta":{"content":"Hello"}}]}"#),
        frame("", r#"{"choices":[{"delta":{"content":", world"}}]}"#),
        frame(
            "",
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":12,"completion_tokens":3}}"#,
        ),
        done_frame(),
    ]
}

fn with_tool_call_frames() -> Vec<Vec<u8>> {
    vec![
        frame("", r#"{"choices":[{"delta":{"role":"assistant"}}]}"#),
        // Opening delta carries id + name.
        frame(
            "",
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_abc","function":{"name":"bash","arguments":"{\"cmd\":"}}]}}]}"#,
        ),
        // Arguments fragmented to exercise partial_json reassembly.
        frame(
            "",
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"echo hi\"}"}}]}}]}"#,
        ),
        frame(
            "",
            r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":18,"completion_tokens":7}}"#,
        ),
        done_frame(),
    ]
}

fn reasoning_frames() -> Vec<Vec<u8>> {
    vec![
        frame("", r#"{"choices":[{"delta":{"role":"assistant"}}]}"#),
        frame(
            "",
            r#"{"choices":[{"delta":{"reasoning_content":"thinking..."}}]}"#,
        ),
        frame("", r#"{"choices":[{"delta":{"content":"answer"}}]}"#),
        frame(
            "",
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":20,"completion_tokens":4}}"#,
        ),
        done_frame(),
    ]
}

fn mid_stream_cancel_frames() -> Vec<Vec<u8>> {
    // One half-frame, then connection close — exercises the parser's
    // `finish()` drain and the consumer's mid-stream error path.
    vec![
        frame("", r#"{"choices":[{"delta":{"role":"assistant"}}]}"#),
        frame("", r#"{"choices":[{"delta":{"content":"partial"}}]}"#),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_token_in_simple_string_content() {
        let msgs = serde_json::json!([
            {"role": "user", "content": "hello PARITY_SCENARIO:reasoning world"}
        ]);
        assert_eq!(detect_scenario(&msgs), Some(Scenario::Reasoning));
    }

    #[test]
    fn detects_token_in_array_content() {
        let msgs = serde_json::json!([
            {"role": "user", "content": [
                {"type": "text", "text": "PARITY_SCENARIO:with_tool_call"}
            ]}
        ]);
        assert_eq!(detect_scenario(&msgs), Some(Scenario::WithToolCall));
    }

    #[test]
    fn unknown_scenario_falls_back_to_simple_text() {
        assert_eq!(Scenario::parse("does_not_exist"), Scenario::SimpleText);
    }

    #[test]
    fn no_token_returns_none() {
        let msgs = serde_json::json!([{"role": "user", "content": "hi"}]);
        assert!(detect_scenario(&msgs).is_none());
    }

    #[test]
    fn skips_messages_without_content_key() {
        // An assistant turn carrying only `tool_calls` (no `content`)
        // must not abort the reverse scan — the token in the earlier
        // user message still wins.
        let msgs = serde_json::json!([
            {"role": "user", "content": "PARITY_SCENARIO:reasoning"},
            {"role": "assistant", "tool_calls": [{"id": "x"}]},
        ]);
        assert_eq!(detect_scenario(&msgs), Some(Scenario::Reasoning));
    }
}

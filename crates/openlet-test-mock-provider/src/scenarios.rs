//! Scenario catalog for the mock OpenAI-compat service.
//!
//! Each variant defines either an SSE byte stream (200 OK) or a
//! JSON error body (>=400). Scenarios are selected by scanning the
//! inbound `messages[].content` for `PARITY_SCENARIO:<name>`.

use std::time::Duration;

/// Magic-token prefix embedded in user-message text to select a scenario.
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
    /// Single `write` tool-call to the real `write` tool, then (on the
    /// re-POST that carries the tool result) a terminal text turn. The
    /// stateless mock can't script a multi-step sequence, so this proves
    /// exactly ONE real dispatch → real LocalFilesystem write → on-disk
    /// state. Detection promotes to the terminal turn once a `role:"tool"`
    /// message is present in the request body.
    FsWriteOnce,
    /// Terminal text turn used as `FsWriteOnce`'s second step. Same byte
    /// shape as `SimpleText` but a distinct variant so the intent is clear.
    FsWriteDone,
}

/// Workspace-relative path the `fs_write_once` scenario writes. Tests
/// assert this file exists on disk with [`FS_WRITE_CONTENT`].
pub const FS_WRITE_PATH: &str = "hello.txt";
/// Exact bytes the `fs_write_once` scenario writes.
pub const FS_WRITE_CONTENT: &str = "hi from openlet";

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
            "fs_write_once" => Self::FsWriteOnce,
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
            Self::FsWriteOnce => Response::sse(fs_write_once_frames()),
            Self::FsWriteDone => Response::sse(fs_write_done_frames()),
        }
    }
}

/// Locate the `PARITY_SCENARIO:<name>` token inside a user message's text
/// content. Walks `messages[]` in reverse so the most
/// recent user turn wins. Returns `None` if no token is present.
#[must_use]
pub fn detect_scenario(messages: &serde_json::Value) -> Option<Scenario> {
    let arr = messages.as_array()?;
    let mut found: Option<Scenario> = None;
    for msg in arr.iter().rev() {
        // Skip messages without a `content` field (e.g. assistant turns
        // carrying only `tool_calls`) rather than aborting the whole
        // scan — earlier messages may still hold the token.
        let Some(content) = msg.get("content") else {
            continue;
        };
        if let Some(text) = content.as_str() {
            if let Some(s) = scan_token(text) {
                found = Some(s);
                break;
            }
        } else if let Some(parts) = content.as_array() {
            for part in parts {
                if let Some(text) = part.get("text").and_then(|v| v.as_str())
                    && let Some(s) = scan_token(text)
                {
                    found = Some(s);
                    break;
                }
            }
            if found.is_some() {
                break;
            }
        }
    }
    // The mock is stateless — each re-POST re-detects from the body. For
    // the single-call `fs_write_once` tier, promote to the terminal
    // `FsWriteDone` once the turn loop has fed the tool result back as a
    // `role:"tool"` message; otherwise turn 2 would re-emit the same write
    // call and loop until max_steps. (Other scenarios are unaffected.)
    if found == Some(Scenario::FsWriteOnce) && has_tool_role_message(arr) {
        return Some(Scenario::FsWriteDone);
    }
    found
}

/// True if any message in the request carries `role:"tool"` — the marker
/// that the turn loop has already executed a tool call and fed its result
/// back into the next LLM input.
fn has_tool_role_message(arr: &[serde_json::Value]) -> bool {
    arr.iter()
        .any(|m| m.get("role").and_then(|r| r.as_str()) == Some("tool"))
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
    Json {
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
    /// 200 OK with a JSON body. Used by the `GET /models` fixture so the
    /// model-catalog path has a network-free response to decode.
    pub(crate) fn ok_json(body: &str) -> Self {
        Self::Json {
            status: 200,
            status_text: "OK",
            body: body.to_string(),
            extra_headers: Vec::new(),
        }
    }
    fn error(
        status: u16,
        status_text: &'static str,
        body: &str,
        extra_headers: &[(&'static str, &str)],
    ) -> Self {
        Self::Json {
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

fn fs_write_once_frames() -> Vec<Vec<u8>> {
    // One `write` tool call carrying the real tool's `path` + `content`
    // args (built from the exported consts so the test asserts the same
    // values). serde handles escaping the args string, which is itself
    // JSON embedded as the `arguments` field value.
    let args = serde_json::json!({
        "path": FS_WRITE_PATH,
        "content": FS_WRITE_CONTENT,
    })
    .to_string();
    let opening = serde_json::json!({
        "choices": [{
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_write_1",
                    "function": { "name": "write", "arguments": args }
                }]
            }
        }]
    })
    .to_string();
    vec![
        frame("", r#"{"choices":[{"delta":{"role":"assistant"}}]}"#),
        frame("", &opening),
        frame(
            "",
            r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":15,"completion_tokens":8}}"#,
        ),
        done_frame(),
    ]
}

fn fs_write_done_frames() -> Vec<Vec<u8>> {
    // Terminal text turn — emitted on the re-POST after the write tool
    // result is fed back, so the turn loop finishes instead of re-issuing
    // the same write call.
    vec![
        frame("", r#"{"choices":[{"delta":{"role":"assistant"}}]}"#),
        frame(
            "",
            r#"{"choices":[{"delta":{"content":"wrote the file"}}]}"#,
        ),
        frame(
            "",
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":20,"completion_tokens":3}}"#,
        ),
        done_frame(),
    ]
}

fn with_tool_call_frames() -> Vec<Vec<u8>> {
    vec![
        frame("", r#"{"choices":[{"delta":{"role":"assistant"}}]}"#),
        // Opening delta carries id + name. Args use the real `bash` tool's
        // `command` field (NOT `cmd`) so the dispatcher's pre-permission
        // arg-parse succeeds and the call reaches the permission gate.
        frame(
            "",
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_abc","function":{"name":"bash","arguments":"{\"command\":"}}]}}]}"#,
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

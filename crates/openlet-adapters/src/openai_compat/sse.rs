//! Byte-stream SSE frame parser.
//!
//! Ported from `claw-code/rust/crates/api/src/sse.rs` with the Anthropic
//! streaming wire shape replaced by raw frame extraction. The parser yields
//! payload strings (the `data:` body of each frame); decoding into the
//! provider-specific JSON shape lives in `wire.rs`.
//!
//! Frame model (per WHATWG EventSource spec):
//! - frames separated by blank line (`\n\n` or `\r\n\r\n`)
//! - lines starting with `:` are comments (heartbeats), skipped
//! - `event: <name>` sets a frame-local event name
//! - `data: <body>` lines are concatenated with `\n`
//! - `data: [DONE]` is the OpenAI-compat terminator (no JSON to decode)

use openlet_core::error::ProviderError;

/// One SSE frame after line-folding. `event` is None unless the upstream
/// emitted an `event:` line (rare for OpenAI-compat).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseFrame {
    pub event: Option<String>,
    pub data: String,
}

impl SseFrame {
    /// True when the frame is the OpenAI-compat stream terminator.
    #[must_use]
    pub fn is_done(&self) -> bool {
        self.data == "[DONE]"
    }

    /// True when the frame is a heartbeat / ping that carries no payload.
    #[must_use]
    pub fn is_heartbeat(&self) -> bool {
        matches!(self.event.as_deref(), Some("ping"))
    }
}

/// Streaming SSE parser. Push raw bytes via `push`; receive parsed frames
/// in order. Drains buffered tail via `finish` once the upstream closes.
#[derive(Debug, Default)]
pub struct SseParser {
    buffer: Vec<u8>,
}

impl SseParser {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Append `chunk` to the internal buffer and parse every complete frame
    /// available. Incomplete trailing bytes stay buffered for the next call.
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<SseFrame>, ProviderError> {
        self.buffer.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(frame_str) = self.next_frame() {
            if let Some(frame) = parse_frame(&frame_str) {
                out.push(frame);
            }
        }
        Ok(out)
    }

    /// Drain any trailing frame the upstream did not terminate with the
    /// blank-line separator. Returns at most one frame.
    pub fn finish(&mut self) -> Result<Vec<SseFrame>, ProviderError> {
        if self.buffer.is_empty() {
            return Ok(Vec::new());
        }
        let trailing = std::mem::take(&mut self.buffer);
        let frame_str = String::from_utf8_lossy(&trailing).into_owned();
        Ok(parse_frame(&frame_str).into_iter().collect())
    }

    fn next_frame(&mut self) -> Option<String> {
        // Split at the EARLIEST separator of either kind. Preferring
        // `\n\n` globally would merge two frames when a `\r\n\r\n`
        // separator appears earlier in the buffer (mixed line endings).
        // For consistent streams only one kind is ever present, so this
        // is identical to picking that kind. A `\r\n\r\n` never contains
        // a bare `\n\n`, so the two never overlap at the same index.
        let lf = self
            .buffer
            .windows(2)
            .position(|w| w == b"\n\n")
            .map(|pos| (pos, 2));
        let crlf = self
            .buffer
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map(|pos| (pos, 4));
        let (pos, sep_len) = match (lf, crlf) {
            (Some(lf), Some(crlf)) => {
                if lf.0 <= crlf.0 {
                    lf
                } else {
                    crlf
                }
            }
            (Some(lf), None) => lf,
            (None, Some(crlf)) => crlf,
            (None, None) => return None,
        };

        let drained: Vec<u8> = self.buffer.drain(..pos + sep_len).collect();
        let body_len = drained.len().saturating_sub(sep_len);
        Some(String::from_utf8_lossy(&drained[..body_len]).into_owned())
    }
}

/// Parse one frame body (already split on blank-line separator). Returns
/// `None` for empty / data-less frames.
fn parse_frame(frame: &str) -> Option<SseFrame> {
    let trimmed = frame.trim();
    if trimmed.is_empty() {
        return None;
    }

    let mut event_name: Option<String> = None;
    let mut data_lines: Vec<&str> = Vec::new();

    for line in trimmed.lines() {
        if line.starts_with(':') {
            continue; // comment / keepalive
        }
        if let Some(name) = line.strip_prefix("event:") {
            event_name = Some(name.trim().to_string());
            continue;
        }
        if let Some(data) = line.strip_prefix("data:") {
            data_lines.push(data.trim_start());
        }
    }

    if data_lines.is_empty() {
        // event: ping with no data, or all-comment frame
        return event_name.map(|e| SseFrame {
            event: Some(e),
            data: String::new(),
        });
    }

    Some(SseFrame {
        event: event_name,
        data: data_lines.join("\n"),
    })
}

#[cfg(test)]
mod tests {
    use super::{SseFrame, SseParser, parse_frame};

    #[test]
    fn parses_single_data_frame() {
        let frame = "data: {\"id\":\"1\"}\n\n";
        let mut p = SseParser::new();
        let frames = p.push(frame.as_bytes()).unwrap();
        assert_eq!(
            frames,
            vec![SseFrame {
                event: None,
                data: "{\"id\":\"1\"}".to_string()
            }]
        );
    }

    #[test]
    fn parses_chunked_frames() {
        let mut p = SseParser::new();
        let part1 = b"data: {\"id\":\"1\",\"choi";
        let part2 = b"ces\":[]}\n\n";
        assert!(p.push(part1).unwrap().is_empty());
        let frames = p.push(part2).unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].data, "{\"id\":\"1\",\"choices\":[]}");
    }

    #[test]
    fn ignores_comments_and_done() {
        let body = ": keepalive\ndata: [DONE]\n\n";
        let mut p = SseParser::new();
        let frames = p.push(body.as_bytes()).unwrap();
        assert_eq!(frames.len(), 1);
        assert!(frames[0].is_done());
    }

    #[test]
    fn ping_event_yields_heartbeat() {
        let body = "event: ping\n\n";
        let frame = parse_frame(body).unwrap();
        assert!(frame.is_heartbeat());
        assert_eq!(frame.data, "");
    }

    #[test]
    fn multi_line_data_joined_with_newline() {
        let frame = "data: line1\ndata: line2\n\n";
        let mut p = SseParser::new();
        let frames = p.push(frame.as_bytes()).unwrap();
        assert_eq!(frames[0].data, "line1\nline2");
    }

    #[test]
    fn handles_crlf_separator() {
        let frame = "data: ok\r\n\r\n";
        let mut p = SseParser::new();
        let frames = p.push(frame.as_bytes()).unwrap();
        assert_eq!(frames[0].data, "ok");
    }

    #[test]
    fn mixed_separators_split_at_earliest() {
        // A CRLF-terminated frame followed by an LF-terminated frame must
        // yield two frames, not one merged frame. Splitting always at the
        // earliest separator of either kind handles this.
        let mut p = SseParser::new();
        let frames = p.push(b"data: a\r\n\r\ndata: b\n\n").unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].data, "a");
        assert_eq!(frames[1].data, "b");
    }

    #[test]
    fn finish_drains_trailing_unterminated_frame() {
        let mut p = SseParser::new();
        // upstream closed mid-frame, no trailing blank line
        p.push(b"data: tail").unwrap();
        let frames = p.finish().unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].data, "tail");
    }
}

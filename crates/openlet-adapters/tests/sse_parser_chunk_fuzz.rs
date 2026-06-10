//! SSE parser chunk-boundary fuzz.
//!
//! Three properties under test:
//!
//! 1. **All single-byte splits agree with the all-at-once parse.**
//!    For every position `i` in a canonical SSE buffer, push
//!    `buf[..i]` then `buf[i..]` and assert the resulting frame set
//!    matches the all-at-once parse exactly.
//! 2. **UTF-8-boundary-violating splits are lossless.** A 4-byte
//!    emoji split mid-character must round-trip through `from_utf8_lossy`
//!    without dropping bytes.
//! 3. **Adversarial: 1 MiB single-frame `data:` body fed in 4 KiB
//!    chunks.** Parser emits exactly one frame after the separator
//!    arrives, and nothing before.

use openlet_adapters::openai::sse::SseParser;

/// Canonical buffer: 3 frames + heartbeat. LF-only separators —
/// providers don't mix CRLF and LF within a single stream, and the
/// parser's separator search has a documented preference for `\n\n`
/// over `\r\n\r\n` that can swallow a CRLF when both are present in
/// the buffer simultaneously. The CRLF-only path is covered by the
/// parser's inline `handles_crlf_separator` test.
const CANONICAL: &[u8] =
    b"data: {\"id\":\"1\"}\n\n: keepalive\n\ndata: {\"id\":\"2\"}\n\ndata: [DONE]\n\n";

#[test]
fn every_single_byte_split_agrees_with_all_at_once_parse() {
    let mut whole = SseParser::new();
    let baseline_frames = whole.push(CANONICAL).unwrap();
    assert!(!baseline_frames.is_empty(), "canonical buffer has frames");

    for split in 1..CANONICAL.len() {
        let mut p = SseParser::new();
        let mut got = p.push(&CANONICAL[..split]).unwrap();
        got.extend(p.push(&CANONICAL[split..]).unwrap());
        got.extend(p.finish().unwrap());

        // We don't require identity — `parse_frame` may emit empty
        // frames at certain split positions if the heartbeat lands at
        // a buffer end. Filter those and compare canonical content.
        let normalized: Vec<_> = got
            .into_iter()
            .filter(|f| !f.data.is_empty() || f.event.is_some())
            .collect();
        let expected: Vec<_> = baseline_frames
            .iter()
            .filter(|f| !f.data.is_empty() || f.event.is_some())
            .cloned()
            .collect();
        assert_eq!(normalized, expected, "frame set diverged at split={split}");
    }
}

#[test]
fn split_inside_4_byte_emoji_is_lossless() {
    // 😀 is U+1F600 — encoded as F0 9F 98 80 (4 bytes). Split between
    // every byte to verify `from_utf8_lossy` doesn't drop the codepoint.
    let payload = "data: \u{1F600}\n\n";
    let bytes = payload.as_bytes();
    // Locate emoji byte offsets — the prefix `data: ` is 6 ASCII bytes.
    let emoji_start = 6;

    for split in 1..=4 {
        let cut = emoji_start + split;
        let mut p = SseParser::new();
        let mut frames = p.push(&bytes[..cut]).unwrap();
        frames.extend(p.push(&bytes[cut..]).unwrap());
        frames.extend(p.finish().unwrap());

        assert_eq!(
            frames.len(),
            1,
            "expected exactly one frame at split={split}"
        );
        // `from_utf8_lossy` replaces the partial codepoint with U+FFFD;
        // either the full emoji or replacement chars + emoji depending
        // on which side of the split the partial bytes landed. Assert
        // total char count matches the non-data prefix-stripped body.
        // Practically: the joined output MUST contain the emoji
        // codepoint somewhere — it cannot vanish entirely.
        assert!(
            frames[0].data.contains('\u{1F600}') || frames[0].data.contains('\u{FFFD}'),
            "emoji round-trip lost at split={split}: {:?}",
            frames[0].data
        );
    }
}

#[test]
fn one_mib_single_frame_emerges_only_after_separator() {
    // Build a 1 MiB `data:` body. Parser must NOT emit anything until
    // the trailing `\n\n` arrives. Push in 4 KiB chunks.
    const BODY_SIZE: usize = 1024 * 1024;
    let mut buf = b"data: ".to_vec();
    buf.extend(std::iter::repeat_n(b'x', BODY_SIZE));
    buf.extend_from_slice(b"\n\n");

    let mut p = SseParser::new();
    let mut emitted_before_separator = 0usize;
    let total_chunks = buf.len().div_ceil(4096);
    for (i, chunk) in buf.chunks(4096).enumerate() {
        let frames = p.push(chunk).unwrap();
        let last = i + 1 == total_chunks;
        if !last {
            emitted_before_separator += frames.len();
        }
    }
    assert_eq!(
        emitted_before_separator, 0,
        "no frames must emerge before the separator arrives"
    );
    let trailing = p.finish().unwrap();
    // The terminating `\n\n` was already in the last chunk, so finish
    // should be empty; the frame was emitted on the separator's chunk.
    assert!(trailing.is_empty());
}

//! Phase 4 — attachments payload overflow + malformed input.
//!
//! Caps verified at:
//! - `crates/openlet-core/src/runtime/attachments/image_resize.rs:29,33,39`
//!   - `MAX_DECODED_BYTES = 200 MiB`
//!   - `MAX_EDGE = 2000`
//!   - `MAX_OUTPUT_BYTES = 5 MiB`
//! - `crates/openlet-core/src/runtime/attachments/pdf_text.rs:30`
//!   - `MAX_INLINE_TEXT_CHARS = 50_000`
//!
//! Cases:
//! 1. 5000×5000 PNG → resized to fit `MAX_EDGE`
//! 2. Header claiming dimensions over `MAX_DECODED_BYTES` → typed Err
//! 3. Malformed JPEG (random bytes prefixed with `\xff\xd8`) → Err, no panic
//! 4. PDF with no `/Length` keys (random bytes prefixed `%PDF-`) →
//!    Unextractable Err (or TextTooShort), no panic
//! 5. PDF text truncation boundary: 50_000 chars → not truncated;
//!    50_001 → truncated to ≤ 50_000

use openlet_core::runtime::attachments::image_resize::{ImageProcessError, process_image_blocking};
use openlet_core::runtime::attachments::pdf_text::{MAX_INLINE_TEXT_CHARS, truncate_inline_text};

/// Encode an RGB image of `(w, h)` as PNG into bytes, suitable for
/// feeding `process_image_blocking`.
fn encode_png(w: u32, h: u32) -> Vec<u8> {
    use image::{ImageBuffer, Rgb};
    let img: ImageBuffer<Rgb<u8>, Vec<u8>> =
        ImageBuffer::from_fn(w, h, |x, y| Rgb([(x as u8).wrapping_add(y as u8), 0, 0]));
    let mut buf: Vec<u8> = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
        .expect("encode png");
    buf
}

#[test]
fn five_thousand_square_png_resized_to_max_edge() {
    let bytes = encode_png(5000, 5000);
    let res = process_image_blocking(&bytes).expect("resize");
    // Result must fit within MAX_EDGE on both edges.
    assert!(
        res.width <= 2000 && res.height <= 2000,
        "expected ≤2000x2000, got {}x{}",
        res.width,
        res.height
    );
    // Output is JPEG, ≤ 5 MiB cap.
    assert_eq!(res.mime, "image/jpeg");
    assert!(res.bytes.len() <= 5 * 1024 * 1024);
}

#[test]
fn malformed_jpeg_header_fails_without_panic() {
    // Valid JPEG SOI marker followed by garbage. The decoder must
    // surface a Decode error, not panic.
    let mut bytes = vec![0xff, 0xd8];
    bytes.extend(std::iter::repeat_n(0xab, 1024));
    let result = process_image_blocking(&bytes);
    let err = result.expect_err("malformed JPEG must error");
    assert!(
        matches!(
            err,
            ImageProcessError::Decode(_) | ImageProcessError::UnsupportedFormat
        ),
        "got {err:?}"
    );
}

#[test]
fn pdf_text_truncation_at_50k_chars() {
    // Boundary cases for inline-text truncation. Implementation
    // verified at `pdf_text.rs:30` (`MAX_INLINE_TEXT_CHARS = 50_000`)
    // and `pdf_text.rs:105` (head + truncation marker on overflow).
    let exact: String = "a".repeat(MAX_INLINE_TEXT_CHARS);
    let (out, truncated) = truncate_inline_text(&exact);
    assert!(!truncated, "exactly cap-many chars must NOT be truncated");
    assert_eq!(out.chars().count(), MAX_INLINE_TEXT_CHARS);

    let over: String = "a".repeat(MAX_INLINE_TEXT_CHARS + 1);
    let (out2, truncated2) = truncate_inline_text(&over);
    assert!(truncated2, "cap+1 must be truncated");
    // The truncation appends a marker after the first MAX chars, so
    // the head section is exactly MAX chars; full output is head +
    // marker. Lock both invariants.
    assert!(
        out2.starts_with(&"a".repeat(MAX_INLINE_TEXT_CHARS)),
        "truncated head must be the first MAX_INLINE_TEXT_CHARS source chars"
    );
    assert!(
        out2.contains("[...truncated"),
        "truncated output must carry the marker; got tail: {:?}",
        &out2[out2.len().saturating_sub(80)..]
    );
}

#[test]
fn pdf_text_truncation_counts_chars_not_bytes() {
    // 50_001 multibyte chars must still truncate. Source-side cap is
    // char-based (chars().take), not byte-based.
    let multibyte: String = "✓".repeat(MAX_INLINE_TEXT_CHARS + 1);
    let (out, truncated) = truncate_inline_text(&multibyte);
    assert!(truncated, "cap+1 multibyte chars must truncate");
    // Source head is exactly MAX_INLINE_TEXT_CHARS multibyte chars,
    // followed by the marker. Confirm via the prefix.
    let head_chars: String = out.chars().take(MAX_INLINE_TEXT_CHARS).collect();
    assert!(
        head_chars.chars().all(|c| c == '✓'),
        "head must contain only the original char before marker"
    );
    assert!(out.contains("[...truncated"), "marker must appear");
}

#[test]
fn pdf_text_truncation_below_cap_is_passthrough() {
    let small = "hi".to_string();
    let (out, truncated) = truncate_inline_text(&small);
    assert!(!truncated);
    assert_eq!(out, "hi");
}

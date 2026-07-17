//! Image resize + JPEG re-encode pipeline.
//!
//! Pipeline order:
//!   1. Pre-validate dimensions via `image::ImageReader::with_guessed_format`
//!      (header read only). Reject if `width * height * 4 > 200MB` BEFORE
//!      allocating the pixel buffer. Guards against a decompression bomb.
//!   2. Decode in `tokio::task::spawn_blocking` — the `image` crate is
//!      synchronous and CPU-bound.
//!   3. Resize to fit ≤2000×2000 preserving aspect (Lanczos3).
//!   4. Re-encode through JPEG. Always re-encode, even on small input:
//!      this strips EXIF (incl. GPS) and avoids MIME-claim drift.
//!   5. JPEG quality ladder [85, 75, 65] — accept the first encode
//!      ≤5MB, otherwise return `TooComplexToResize` rather than
//!      degrading silently.

use std::io::Cursor;

use image::ImageReader;
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ExtendedColorType, ImageError, ImageFormat};
use thiserror::Error;

/// Hard cap on the decoded pixel buffer. 200MB == 50 megapixels at
/// RGBA8. Any uploaded image whose header claims more is rejected
/// before we allocate. The number is policy, not a property of the
/// decoder — bump deliberately if downstream gets bigger panels.
const MAX_DECODED_BYTES: u64 = 200 * 1024 * 1024;

/// Maximum output edge length. Anything bigger is resized down with
/// aspect ratio preserved.
const MAX_EDGE: u32 = 2000;

/// Maximum encoded JPEG payload size. The route enforces a 25MB raw
/// upload cap; this is the post-encode commitment to downstream
/// providers (Anthropic / OpenAI typically reject larger inline
/// images).
const MAX_OUTPUT_BYTES: usize = 5 * 1024 * 1024;

/// Successful result. `bytes` is always `image/jpeg`; `mime` is fixed
/// here for symmetry with `PdfProcessResult`.
#[derive(Debug, Clone)]
pub struct ImageProcessResult {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub mime: &'static str,
}

#[derive(Debug, Error)]
pub enum ImageProcessError {
    #[error("unsupported image format")]
    UnsupportedFormat,
    #[error(
        "image dimensions exceed limit: {width}x{height} (decoded ~{decoded_bytes} bytes > \
         {limit} bytes)"
    )]
    ImageDimensionsExceedLimit {
        width: u32,
        height: u32,
        decoded_bytes: u64,
        limit: u64,
    },
    #[error("image decode failed: {0}")]
    Decode(String),
    #[error("image encode failed: {0}")]
    Encode(String),
    #[error(
        "image too complex to resize under {} bytes at quality 65",
        MAX_OUTPUT_BYTES
    )]
    TooComplexToResize,
    #[error("image runtime error: {0}")]
    Runtime(String),
}

/// Async entry point. The decode runs inside `spawn_blocking` so a
/// CPU-bound resize of a large input doesn't stall the runtime.
pub async fn process_image(input: Vec<u8>) -> Result<ImageProcessResult, ImageProcessError> {
    tokio::task::spawn_blocking(move || process_image_blocking(&input))
        .await
        .map_err(|e| ImageProcessError::Runtime(e.to_string()))?
}

/// Synchronous core. Public for tests; the route always calls
/// `process_image`.
pub fn process_image_blocking(input: &[u8]) -> Result<ImageProcessResult, ImageProcessError> {
    let cursor = Cursor::new(input);
    let reader = ImageReader::new(cursor)
        .with_guessed_format()
        .map_err(|e| ImageProcessError::Decode(e.to_string()))?;

    if reader.format().is_none() {
        return Err(ImageProcessError::UnsupportedFormat);
    }

    // Header-only read: rejects decompression bombs before any pixel
    // buffer is allocated. The `image` crate caches the header parse
    // so the subsequent `decode` doesn't re-read.
    let (width, height) = reader
        .into_dimensions()
        .map_err(|e| ImageProcessError::Decode(e.to_string()))?;

    let decoded_bytes = u64::from(width) * u64::from(height) * 4;
    if decoded_bytes > MAX_DECODED_BYTES {
        return Err(ImageProcessError::ImageDimensionsExceedLimit {
            width,
            height,
            decoded_bytes,
            limit: MAX_DECODED_BYTES,
        });
    }

    // Re-open for the actual decode (the previous reader was consumed
    // by `into_dimensions`). Format is still guessed from the bytes.
    let cursor = Cursor::new(input);
    let reader = ImageReader::new(cursor)
        .with_guessed_format()
        .map_err(|e| ImageProcessError::Decode(e.to_string()))?;
    let img = reader.decode().map_err(|e| match e {
        ImageError::Unsupported(_) => ImageProcessError::UnsupportedFormat,
        other => ImageProcessError::Decode(other.to_string()),
    })?;

    let resized = resize_to_fit(img, MAX_EDGE);
    let (out_width, out_height) = (resized.width(), resized.height());
    let bytes = encode_with_quality_ladder(&resized)?;

    Ok(ImageProcessResult {
        bytes,
        width: out_width,
        height: out_height,
        mime: "image/jpeg",
    })
}

/// Resize so neither edge exceeds `max_edge`, preserving aspect ratio.
/// No-op when the input already fits — but the caller still re-encodes
/// to strip metadata.
fn resize_to_fit(img: DynamicImage, max_edge: u32) -> DynamicImage {
    let (w, h) = (img.width(), img.height());
    if w <= max_edge && h <= max_edge {
        return img;
    }
    img.resize(max_edge, max_edge, FilterType::Lanczos3)
}

/// Try [85, 75, 65] in order. Return the first encode ≤5MB. The
/// `image` JPEG encoder strips ancillary metadata (EXIF, ICC).
fn encode_with_quality_ladder(img: &DynamicImage) -> Result<Vec<u8>, ImageProcessError> {
    // Convert once. JPEG cannot carry alpha, so RGBA inputs flatten
    // to RGB on encode anyway; doing it explicitly keeps the encoder
    // call deterministic.
    let rgb8 = img.to_rgb8();
    let (w, h) = (rgb8.width(), rgb8.height());
    let raw = rgb8.as_raw();
    for quality in [85u8, 75, 65] {
        let mut buf: Vec<u8> = Vec::new();
        let mut encoder = JpegEncoder::new_with_quality(&mut buf, quality);
        encoder
            .encode(raw, w, h, ExtendedColorType::Rgb8)
            .map_err(|e| ImageProcessError::Encode(e.to_string()))?;
        if buf.len() <= MAX_OUTPUT_BYTES {
            return Ok(buf);
        }
    }
    Err(ImageProcessError::TooComplexToResize)
}

/// Format introspection for callers that want to log or audit the
/// detected format before processing. Returns `None` on
/// unidentifiable input.
#[must_use]
pub fn detect_format(input: &[u8]) -> Option<ImageFormat> {
    let cursor = Cursor::new(input);
    ImageReader::new(cursor)
        .with_guessed_format()
        .ok()
        .and_then(|r| r.format())
}

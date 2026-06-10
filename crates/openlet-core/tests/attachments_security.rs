//! Security-focused tests for the image and PDF attachment pipelines.

use openlet_core::runtime::attachments::image_resize::{ImageProcessError, process_image_blocking};
use openlet_core::runtime::attachments::pdf_text::{
    MAX_INLINE_TEXT_CHARS, substitute_artifact_id, truncate_inline_text,
};

/// Header-only dimension validation rejects a decompression
/// bomb without allocating the pixel buffer. We hand-craft a minimal
/// PNG header claiming 100000×100000 pixels (≈40GB decoded) and
/// verify the pre-validation kicks in.
#[test]
fn image_dim_overflow_rejected_pre_alloc() {
    let bomb = synthetic_png_with_dimensions(100_000, 100_000);
    let err = process_image_blocking(&bomb).expect_err("expected pre-alloc rejection");
    match err {
        ImageProcessError::ImageDimensionsExceedLimit {
            width,
            height,
            decoded_bytes,
            limit,
        } => {
            assert_eq!(width, 100_000);
            assert_eq!(height, 100_000);
            assert!(
                decoded_bytes > limit,
                "decoded={decoded_bytes} limit={limit}"
            );
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

/// Re-encoding through JPEG strips EXIF. We feed a JPEG with
/// an EXIF block (containing a fake GPS marker string) and verify
/// the output bytes do NOT contain the marker.
#[test]
fn image_resize_strips_exif() {
    let exif_marker = b"GPS_LEAK_MARKER_42";
    let input = synthetic_jpeg_with_exif(exif_marker);
    let result = process_image_blocking(&input).expect("process_image succeeds");
    assert_eq!(result.mime, "image/jpeg");
    assert!(
        !result
            .bytes
            .windows(exif_marker.len())
            .any(|w| w == exif_marker),
        "exif marker survived re-encode (found in output bytes)"
    );
}

/// Inline truncation marker present at exactly the cap and
/// the head still substitutable with a concrete artifact id.
#[test]
fn pdf_extract_truncates_at_50k() {
    // Synthetic 200K-char source — `truncate_inline_text` is the
    // public substring-cap helper used by `process_pdf` after the
    // `pdf-extract` parse. Testing it directly avoids fixture-PDF
    // fragility while still exercising the policy.
    let source: String = "x".repeat(200_000);
    let (truncated, was_truncated) = truncate_inline_text(&source);
    assert!(was_truncated);
    assert!(truncated.contains("[...truncated, full text in artifact <id>]"));
    let head_len = truncated.chars().take_while(|&c| c == 'x').count();
    assert_eq!(head_len, MAX_INLINE_TEXT_CHARS);

    // Substituting the placeholder with a concrete id gives the
    // caller a frontend-renderable string without re-running the
    // pipeline.
    let stamped = substitute_artifact_id(&truncated, "pdf-deadbeef");
    assert!(stamped.contains("artifact pdf-deadbeef"));
    assert!(!stamped.contains("<id>"));
}

/// `truncate_inline_text` is a no-op when input is already small.
/// Guard rail for the `truncated` flag so the route doesn't claim
/// truncation on tiny PDFs.
#[test]
fn pdf_extract_does_not_truncate_when_under_cap() {
    let source = "small body".to_string();
    let (out, was_truncated) = truncate_inline_text(&source);
    assert!(!was_truncated);
    assert_eq!(out, source);
}

/// Synthetic PNG bomb: a real (decodable-by-header) PNG whose IHDR
/// has been patched to claim massive dimensions. The IDAT chunk
/// (whose decoder would fail post-header) is left intact because
/// `image::ImageReader::into_dimensions()` validates structural
/// presence of IDAT but reads dimensions from IHDR — so patching
/// IHDR is sufficient to trigger our header-only pre-validation.
fn synthetic_png_with_dimensions(width: u32, height: u32) -> Vec<u8> {
    use image::{ImageFormat, RgbImage};
    use std::io::Cursor;
    // Build a real 1×1 PNG.
    let img = RgbImage::from_pixel(1, 1, image::Rgb([0, 0, 0]));
    let mut buf: Vec<u8> = Vec::new();
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
        .expect("baseline png encode");

    // PNG layout: 8-byte signature + chunks. IHDR is the first chunk.
    //   [0..8]  signature
    //   [8..12] IHDR length (always 13)
    //   [12..16] "IHDR"
    //   [16..20] width (BE)
    //   [20..24] height (BE)
    //   [24]    bit depth
    //   [25]    color type
    //   [26]    compression method
    //   [27]    filter method
    //   [28]    interlace
    //   [29..33] CRC over [12..29]
    buf[16..20].copy_from_slice(&width.to_be_bytes());
    buf[20..24].copy_from_slice(&height.to_be_bytes());
    let crc = crc32_ieee(&buf[12..29]);
    buf[29..33].copy_from_slice(&crc.to_be_bytes());
    buf
}

/// CRC-32 (IEEE 802.3, polynomial 0xEDB88320) — same variant PNG
/// uses. Inline implementation keeps the test self-contained without
/// pulling `crc32fast` as a dev-dep.
fn crc32_ieee(data: &[u8]) -> u32 {
    let mut crc: u32 = !0;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB88320 & mask);
        }
    }
    !crc
}

/// Hand-rolled JPEG with a marker string embedded in an APP1/EXIF
/// segment. We deliberately keep the file tiny (a 1×1 image) so the
/// full pipeline runs end-to-end and we can assert the marker is
/// NOT present in the re-encoded output.
fn synthetic_jpeg_with_exif(marker: &[u8]) -> Vec<u8> {
    use image::{DynamicImage, ImageFormat, RgbImage};
    use std::io::Cursor;
    // Render a 64×64 JPEG via the `image` crate so the bytes are a
    // valid baseline. Then splice an APP1 segment containing the
    // marker into the file so re-decode preserves it (until the
    // re-encode strips it).
    let img = DynamicImage::ImageRgb8(RgbImage::from_pixel(64, 64, image::Rgb([200, 100, 50])));
    let mut buf: Vec<u8> = Vec::new();
    img.write_to(&mut Cursor::new(&mut buf), ImageFormat::Jpeg)
        .expect("baseline jpeg encode");

    // Inject APP1 segment after the SOI (first 2 bytes). Layout:
    //   FF E1 <length-be-u16> "Exif\0\0" marker
    let mut app1 = vec![0xFF, 0xE1];
    let payload_len = (2 + 6 + marker.len()) as u16;
    app1.extend_from_slice(&payload_len.to_be_bytes());
    app1.extend_from_slice(b"Exif\0\0");
    app1.extend_from_slice(marker);

    let mut spliced = Vec::with_capacity(buf.len() + app1.len());
    spliced.extend_from_slice(&buf[..2]);
    spliced.extend_from_slice(&app1);
    spliced.extend_from_slice(&buf[2..]);
    spliced
}

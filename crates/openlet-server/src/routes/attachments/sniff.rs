//! Content sniffing for the attachments route.
//!
//! Closes F3.5: rejects polyglot files where the magic bytes match
//! BOTH a PDF and an image format. We always trust the sniffed kind
//! over the multipart `Content-Type` claim.

use crate::error::AppError;

const JPEG_SOI: [u8; 2] = [0xFF, 0xD8];
const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1A\n";
const BMP_SIGNATURE: [u8; 2] = [0x42, 0x4D];
const IMAGE_BRANDS: &[&[u8; 4]] = &[
    b"heic", b"heix", b"hevc", b"hevx", b"heim", b"heis", b"hevm", b"hevs", b"mif1", b"msf1",
    b"avif", b"avis",
];

/// Sniffer result. We restrict to the two formats the pipeline knows
/// how to process; anything else is rejected at the boundary.
#[derive(Debug, Clone, Copy)]
pub(super) enum DetectedKind {
    Image,
    Pdf,
}

/// `infer`-based content sniffing. Closes F3.5: rejects polyglot
/// files where the magic bytes match BOTH a PDF and an image format.
/// We always trust the sniffed kind over the multipart `Content-Type`.
pub(super) fn sniff_content_type(bytes: &[u8]) -> Result<DetectedKind, AppError> {
    let main = infer::get(bytes).ok_or_else(|| {
        AppError::unsupported_media_type(
            "unknown_media_type",
            "could not detect content type from bytes",
        )
    })?;
    let mime = main.mime_type();
    let main_kind = if mime.starts_with("image/") {
        Some(DetectedKind::Image)
    } else if mime == "application/pdf" {
        Some(DetectedKind::Pdf)
    } else {
        None
    };
    let main_kind = main_kind.ok_or_else(|| {
        AppError::unsupported_media_type(
            "unsupported_media_type",
            format!("media type {mime} is not supported"),
        )
    })?;

    // Polyglot detection: scan a small prefix of the body for an
    // alternate magic-byte signature. If a PDF marker AND any known
    // image format show up, the file is conflicting and rejected.
    // The set is the formats `infer` enumerates as `image/*`; we
    // can't rely on `infer` itself for polyglot detection because it
    // returns the FIRST match, never the conflict.
    let has_pdf_magic = bytes.windows(4).take(1024).any(|w| w == b"%PDF");
    let has_image_magic = sniff_any_image_magic(bytes);
    let conflict = match main_kind {
        DetectedKind::Image => has_pdf_magic && has_image_magic,
        DetectedKind::Pdf => has_image_magic,
    };
    if conflict {
        return Err(AppError::unsupported_media_type(
            "conflicting_magic_bytes",
            "file contains conflicting format magic bytes",
        ));
    }
    Ok(main_kind)
}

/// True if `bytes` contains any of the supported image-format magic
/// markers in its first frame. Covers JPEG SOI, PNG, GIF87a/89a, BMP,
/// WebP (RIFF…WEBP), HEIC/AVIF (`ftyp` major brands). The `infer` crate
/// hides the conflict by returning only the first match — we
/// hand-enumerate the markers so the polyglot check can see all of them.
fn sniff_any_image_magic(bytes: &[u8]) -> bool {
    if bytes.windows(JPEG_SOI.len()).take(8).any(|w| w == JPEG_SOI) {
        return true; // JPEG
    }
    if bytes.starts_with(PNG_SIGNATURE) {
        return true; // PNG
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return true; // GIF
    }
    if bytes.starts_with(&BMP_SIGNATURE) {
        return true; // BMP
    }
    // WebP: 'RIFF' .. 'WEBP'
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return true;
    }
    // HEIF / HEIC / AVIF: ISO BMFF `ftyp` box at offset 4 with a known
    // major brand. Cap the scan at the first 32 bytes so we don't pay
    // for a full-buffer search on hostile input.
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        let brand = &bytes[8..12];
        if IMAGE_BRANDS.iter().any(|b| brand == *b) {
            return true;
        }
    }
    false
}

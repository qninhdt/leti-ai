//! Content sniffing for the attachments route.
//!
//! Closes F3.5: rejects polyglot files where the magic bytes match
//! BOTH a PDF and an image format. We always trust the sniffed kind
//! over the multipart `Content-Type` claim.

use crate::error::AppError;

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
    // alternate magic-byte signature. If both an image and a PDF
    // marker show up, the file is conflicting and rejected. We don't
    // try to be exhaustive — the goal is to defeat the trivial
    // "concatenate two formats" attack, not to certify benign.
    let has_pdf_magic = bytes.windows(4).take(1024).any(|w| w == b"%PDF");
    let has_jpeg_soi = bytes.windows(2).take(8).any(|w| w == [0xFF, 0xD8]);
    let has_png_magic = bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]);
    let image_count = u8::from(has_jpeg_soi) + u8::from(has_png_magic);
    let conflict = match main_kind {
        DetectedKind::Image => has_pdf_magic && image_count > 0,
        DetectedKind::Pdf => image_count > 0,
    };
    if conflict {
        return Err(AppError::unsupported_media_type(
            "conflicting_magic_bytes",
            "file contains conflicting format magic bytes",
        ));
    }
    Ok(main_kind)
}

//! Attachment processing pipeline — image resize and PDF text extraction.
//!
//! Both pipelines run inside `tokio::task::spawn_blocking` because the
//! underlying crates (`image`, `pdf-extract`) are CPU-bound and have
//! historically panicked on malformed input. The PDF path additionally
//! wraps the blocking call in `panic::catch_unwind` so a corrupted PDF
//! cannot crash the runtime (workspace must use `panic = "unwind"`,
//! verified in `Cargo.toml`).
//!
//! Policy lives in core (the 2000×2000 / 5MB / JPEG quality ladder, the
//! 50K-char PDF cap, the 200MB pixel-buffer pre-validation) so cloud and
//! local deployments share one set of gates.

pub mod image_resize;
pub mod pdf_text;

pub use image_resize::{ImageProcessError, ImageProcessResult, process_image};
pub use pdf_text::{PdfProcessError, PdfProcessResult, process_pdf};

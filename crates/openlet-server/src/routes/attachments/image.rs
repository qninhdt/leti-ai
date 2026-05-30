//! Image processing helpers for the attachments route.

use bytes::Bytes;
use openlet_core::runtime::attachments::{ImageProcessError, process_image};
use openlet_core::types::event::AttachmentKind;
use openlet_core::types::part::{Part, PartId};
use openlet_core::types::session::SessionId;
use uuid::Uuid;

use crate::app_state::AppState;
use crate::error::AppError;

pub(super) async fn process_and_persist_image(
    state: &AppState,
    sid: SessionId,
    bytes: Vec<u8>,
) -> Result<(AttachmentKind, String, String, String, Part), AppError> {
    let result = process_image(bytes).await.map_err(map_image_error)?;
    let artifact_id = format!("img-{}", Uuid::new_v4());
    let key = format!("attachments/{artifact_id}.jpg");
    let bytes_for_store = Bytes::from(result.bytes.clone());
    state
        .artifacts
        .put(sid, &key, bytes_for_store)
        .await
        .map_err(|e| AppError::internal("artifact_put_failed", e.to_string()))?;
    let summary = format!(
        "image/jpeg {}x{} ({} bytes)",
        result.width,
        result.height,
        result.bytes.len()
    );
    let part = Part::Image {
        id: PartId::new(),
        artifact_id: artifact_id.clone(),
        mime: "image/jpeg".into(),
        width: result.width,
        height: result.height,
    };
    Ok((
        AttachmentKind::Image,
        "image/jpeg".into(),
        artifact_id,
        summary,
        part,
    ))
}

pub(super) fn map_image_error(e: ImageProcessError) -> AppError {
    match e {
        ImageProcessError::UnsupportedFormat => AppError::unsupported_media_type(
            "unsupported_image_format",
            "image format not supported",
        ),
        ImageProcessError::ImageDimensionsExceedLimit { .. }
        | ImageProcessError::TooComplexToResize => {
            AppError::unprocessable_entity("image_too_large", e.to_string())
        }
        ImageProcessError::Decode(_) | ImageProcessError::Encode(_) => {
            AppError::unprocessable_entity("image_decode_failed", e.to_string())
        }
        ImageProcessError::Runtime(_) => AppError::internal("image_runtime", e.to_string()),
    }
}

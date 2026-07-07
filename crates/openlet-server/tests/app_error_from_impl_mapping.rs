//! Exhaustive HTTP-status mapping tests for `AppError`'s `From<*Error>`
//! impls. Drift here = unstable API → must be caught at PR time.
//!
//! For each `*Error` variant constructed in `error.rs` `From` impls,
//! assert:
//!   - axum response status matches the documented mapping
//!   - response body slug (the `code` field) matches the documented
//!     `&'static str` literal
//!
//! When a new `*Error` variant is added, this test will FAIL to compile
//! (non-exhaustive match) — forcing a deliberate slug + status review.

use axum::body::to_bytes;
use axum::response::IntoResponse;
use openlet_core::error::{
    ArtifactError, ConfigError, EventError, MemoryError, PermissionError, ProviderError, ToolError,
};
use openlet_server::AppError;

async fn status_and_code(err: AppError) -> (u16, String) {
    let resp = err.into_response();
    let status = resp.status().as_u16();
    let body = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let code = v
        .get("code")
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();
    (status, code)
}

#[tokio::test]
async fn memory_error_session_not_found_maps_to_404_session_not_found() {
    let app: AppError = MemoryError::SessionNotFound.into();
    assert_eq!(
        status_and_code(app).await,
        (404, "session_not_found".into())
    );
}

#[tokio::test]
async fn memory_error_message_not_found_maps_to_404_message_not_found() {
    let app: AppError = MemoryError::MessageNotFound.into();
    assert_eq!(
        status_and_code(app).await,
        (404, "message_not_found".into())
    );
}

#[tokio::test]
async fn memory_error_io_maps_to_500_memory_io() {
    let app: AppError = MemoryError::Io("disk full".into()).into();
    assert_eq!(status_and_code(app).await, (500, "memory_io".into()));
}

#[tokio::test]
async fn memory_error_unimplemented_maps_to_500_memory_unimplemented() {
    let app: AppError = MemoryError::Unimplemented.into();
    assert_eq!(
        status_and_code(app).await,
        (500, "memory_unimplemented".into())
    );
}

#[tokio::test]
async fn artifact_error_not_found_maps_to_404_artifact_not_found() {
    let app: AppError = ArtifactError::NotFound("blob.bin".into()).into();
    assert_eq!(
        status_and_code(app).await,
        (404, "artifact_not_found".into())
    );
}

#[tokio::test]
async fn artifact_error_io_maps_to_500_artifact_io() {
    let app: AppError = ArtifactError::Io("write fail".into()).into();
    assert_eq!(status_and_code(app).await, (500, "artifact_io".into()));
}

#[tokio::test]
async fn artifact_error_unimplemented_maps_to_500_artifact_unimplemented() {
    let app: AppError = ArtifactError::Unimplemented.into();
    assert_eq!(
        status_and_code(app).await,
        (500, "artifact_unimplemented".into())
    );
}

#[tokio::test]
async fn event_error_bus_closed_maps_to_500_event_bus_closed() {
    let app: AppError = EventError::BusClosed.into();
    assert_eq!(status_and_code(app).await, (500, "event_bus_closed".into()));
}

#[tokio::test]
async fn event_error_cursor_too_far_behind_maps_to_409_cursor_too_far_behind() {
    let app: AppError = EventError::CursorTooFarBehind {
        requested: 100,
        tip: 1000,
        window: 500,
    }
    .into();
    assert_eq!(
        status_and_code(app).await,
        (409, "cursor_too_far_behind".into())
    );
}

#[tokio::test]
async fn event_error_io_maps_to_500_event_io() {
    let app: AppError = EventError::Io("repo io".into()).into();
    assert_eq!(status_and_code(app).await, (500, "event_io".into()));
}

#[tokio::test]
async fn event_error_unimplemented_maps_to_500_event_unimplemented() {
    let app: AppError = EventError::Unimplemented.into();
    assert_eq!(
        status_and_code(app).await,
        (500, "event_unimplemented".into())
    );
}

#[tokio::test]
async fn permission_error_ask_not_found_maps_to_404_ask_not_found() {
    let app: AppError = PermissionError::AskNotFound.into();
    assert_eq!(status_and_code(app).await, (404, "ask_not_found".into()));
}

#[tokio::test]
async fn permission_error_ask_expired_maps_to_404_ask_expired() {
    let app: AppError = PermissionError::AskExpired.into();
    assert_eq!(status_and_code(app).await, (404, "ask_expired".into()));
}

#[tokio::test]
async fn permission_error_timeout_maps_to_409_ask_timeout() {
    let app: AppError = PermissionError::Timeout.into();
    assert_eq!(status_and_code(app).await, (409, "ask_timeout".into()));
}

#[tokio::test]
async fn permission_error_unsupported_maps_to_400_unsupported_scope() {
    let app: AppError = PermissionError::Unsupported("range scope".into()).into();
    assert_eq!(
        status_and_code(app).await,
        (400, "unsupported_scope".into())
    );
}

#[tokio::test]
async fn permission_error_io_maps_to_500_permission_io() {
    let app: AppError = PermissionError::Io("disk".into()).into();
    assert_eq!(status_and_code(app).await, (500, "permission_io".into()));
}

#[tokio::test]
async fn permission_error_unimplemented_maps_to_500_permission_unimplemented() {
    let app: AppError = PermissionError::Unimplemented.into();
    assert_eq!(
        status_and_code(app).await,
        (500, "permission_unimplemented".into())
    );
}

#[tokio::test]
async fn config_error_invalid_maps_to_400_config_invalid() {
    let app: AppError = ConfigError::Invalid("bad threshold".into()).into();
    assert_eq!(status_and_code(app).await, (400, "config_invalid".into()));
}

#[tokio::test]
async fn config_error_io_maps_to_500_config_io() {
    let app: AppError = ConfigError::Io("permission denied".into()).into();
    assert_eq!(status_and_code(app).await, (500, "config_io".into()));
}

#[tokio::test]
async fn provider_error_missing_credentials_maps_to_401_provider_auth() {
    let app: AppError = ProviderError::MissingCredentials {
        provider: "openrouter",
        env_var: "OPENAI_API_KEY",
    }
    .into();
    assert_eq!(status_and_code(app).await, (401, "provider_auth".into()));
}

#[tokio::test]
async fn provider_error_auth_maps_to_401_provider_auth() {
    let app: AppError = ProviderError::Auth("401 body".into()).into();
    assert_eq!(status_and_code(app).await, (401, "provider_auth".into()));
}

#[tokio::test]
async fn provider_error_rate_limit_maps_to_429_provider_rate_limit() {
    let app: AppError = ProviderError::RateLimit {
        retry_after_ms: 5_000,
    }
    .into();
    assert_eq!(
        status_and_code(app).await,
        (429, "provider_rate_limit".into())
    );
}

#[tokio::test]
async fn provider_error_network_maps_to_502_provider_network() {
    let app: AppError = ProviderError::Network("connection refused".into()).into();
    assert_eq!(status_and_code(app).await, (502, "provider_network".into()));
}

#[tokio::test]
async fn provider_error_decode_maps_to_502_provider_decode() {
    let app: AppError = ProviderError::Decode("bad json".into()).into();
    assert_eq!(status_and_code(app).await, (502, "provider_decode".into()));
}

#[tokio::test]
async fn provider_error_context_window_exceeded_maps_to_413_context_window() {
    let app: AppError = ProviderError::ContextWindowExceeded {
        used: 200_000,
        limit: 128_000,
    }
    .into();
    assert_eq!(status_and_code(app).await, (413, "context_window".into()));
}

#[tokio::test]
async fn provider_error_cancelled_maps_to_409_provider_cancelled() {
    let app: AppError = ProviderError::Cancelled.into();
    assert_eq!(
        status_and_code(app).await,
        (409, "provider_cancelled".into())
    );
}

#[tokio::test]
async fn provider_error_unimplemented_maps_to_500_provider_unimplemented() {
    let app: AppError = ProviderError::Unimplemented.into();
    assert_eq!(
        status_and_code(app).await,
        (500, "provider_unimplemented".into())
    );
}

// ToolError → all variants map to 400 with the FailureClass slug.
#[tokio::test]
async fn tool_error_path_outside_workspace_maps_to_400_with_class_slug() {
    let app: AppError = ToolError::PathOutsideWorkspace("/etc/passwd".into()).into();
    let (status, code) = status_and_code(app).await;
    assert_eq!(status, 400);
    assert_eq!(code, "tool_path_outside_workspace");
}

#[tokio::test]
async fn tool_error_permission_denied_maps_to_400_with_class_slug() {
    let app: AppError = ToolError::PermissionDenied("denied".into()).into();
    let (status, code) = status_and_code(app).await;
    assert_eq!(status, 400);
    assert_eq!(code, "tool_permission_denied");
}

#[tokio::test]
async fn tool_error_read_before_write_maps_to_400_with_class_slug() {
    let app: AppError = ToolError::ReadBeforeWriteRequired("foo.rs".into()).into();
    let (status, code) = status_and_code(app).await;
    assert_eq!(status, 400);
    assert_eq!(code, "tool_read_before_write");
}

#[tokio::test]
async fn tool_error_binary_file_maps_to_400_with_class_slug() {
    let app: AppError = ToolError::BinaryFile("a.png".into()).into();
    let (status, code) = status_and_code(app).await;
    assert_eq!(status, 400);
    assert_eq!(code, "tool_binary_file");
}

#[tokio::test]
async fn tool_error_file_too_large_maps_to_400_with_class_slug() {
    let app: AppError = ToolError::FileTooLarge {
        path: "big".into(),
        bytes: 100_000_000,
        limit: 8_000_000,
    }
    .into();
    let (status, code) = status_and_code(app).await;
    assert_eq!(status, 400);
    assert_eq!(code, "tool_file_too_large");
}

#[tokio::test]
async fn tool_error_not_found_maps_to_400_with_class_slug() {
    let app: AppError = ToolError::NotFound("frobnicate".into()).into();
    let (status, code) = status_and_code(app).await;
    assert_eq!(status, 400);
    assert_eq!(code, "tool_not_found");
}

#[tokio::test]
async fn tool_error_invalid_input_maps_to_400_with_class_slug() {
    let app: AppError = ToolError::InvalidInput("nope".into()).into();
    let (status, code) = status_and_code(app).await;
    assert_eq!(status, 400);
    assert_eq!(code, "tool_invalid_input");
}

#[tokio::test]
async fn tool_error_timeout_maps_to_400_with_class_slug() {
    let app: AppError = ToolError::Timeout.into();
    let (status, code) = status_and_code(app).await;
    assert_eq!(status, 400);
    assert_eq!(code, "tool_timeout");
}

#[tokio::test]
async fn tool_error_io_maps_to_400_with_class_slug() {
    let app: AppError = ToolError::Io("bash spawn".into()).into();
    let (status, code) = status_and_code(app).await;
    assert_eq!(status, 400);
    assert_eq!(code, "tool_io");
}

#[tokio::test]
async fn tool_error_not_allowed_in_agent_maps_to_400_with_class_slug() {
    let app: AppError = ToolError::NotAllowedInAgent {
        tool: "bash".into(),
        agent: "indexer".into(),
    }
    .into();
    let (status, code) = status_and_code(app).await;
    assert_eq!(status, 400);
    assert_eq!(code, "tool_not_allowed_in_agent");
}

#[tokio::test]
async fn tool_error_unimplemented_maps_to_400_with_class_slug() {
    let app: AppError = ToolError::Unimplemented.into();
    let (status, code) = status_and_code(app).await;
    assert_eq!(status, 400);
    assert_eq!(code, "tool_unimplemented");
}

/// Compile-time exhaustiveness gate: when a new `*Error` variant is
/// added without a corresponding `From<*Error>` mapping, this match
/// will fail to compile (the variants enumerated below come from a
/// `match` over the actual enum so non-exhaustive will block CI).
#[allow(dead_code)]
fn variant_exhaustiveness_compile_gate() {
    fn _memory(e: MemoryError) -> AppError {
        match e {
            MemoryError::SessionNotFound
            | MemoryError::MessageNotFound
            | MemoryError::Io(_)
            | MemoryError::Unimplemented => e.into(),
        }
    }
    fn _artifact(e: ArtifactError) -> AppError {
        match e {
            ArtifactError::NotFound(_) | ArtifactError::Io(_) | ArtifactError::Unimplemented => {
                e.into()
            }
        }
    }
    fn _event(e: EventError) -> AppError {
        match e {
            EventError::BusClosed
            | EventError::CursorTooFarBehind { .. }
            | EventError::Io(_)
            | EventError::Unimplemented => e.into(),
        }
    }
    fn _permission(e: PermissionError) -> AppError {
        match e {
            PermissionError::AskNotFound
            | PermissionError::AskExpired
            | PermissionError::Timeout
            | PermissionError::Unsupported(_)
            | PermissionError::Io(_)
            | PermissionError::Unimplemented => e.into(),
        }
    }
    fn _config(e: ConfigError) -> AppError {
        match e {
            ConfigError::Invalid(_) | ConfigError::Io(_) => e.into(),
        }
    }
    fn _provider(e: ProviderError) -> AppError {
        match e {
            ProviderError::MissingCredentials { .. }
            | ProviderError::Auth(_)
            | ProviderError::RateLimit { .. }
            | ProviderError::Network(_)
            | ProviderError::Decode(_)
            | ProviderError::ContextWindowExceeded { .. }
            | ProviderError::Cancelled
            | ProviderError::Unimplemented => e.into(),
        }
    }
    fn _tool(e: ToolError) -> AppError {
        match e {
            ToolError::PathOutsideWorkspace(_)
            | ToolError::PermissionDenied(_)
            | ToolError::ReadBeforeWriteRequired(_)
            | ToolError::BinaryFile(_)
            | ToolError::FileTooLarge { .. }
            | ToolError::NotFound(_)
            | ToolError::InvalidInput(_)
            | ToolError::Timeout
            | ToolError::Io(_)
            | ToolError::NotAllowedInAgent { .. }
            | ToolError::Unimplemented => e.into(),
        }
    }
}

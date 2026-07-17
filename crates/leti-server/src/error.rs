//! Centralized HTTP error type. Each variant maps to a stable
//! `&'static str` slug + a status code; routes return `AppError` and
//! axum converts via `IntoResponse`.
//!
//! No `Other` variants — every error must be a typed
//! variant with a closed-set slug.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use leti_core::error::{
    ArtifactError, ConfigError, EventError, FsError, MemoryError, PermissionError, ProviderError,
    ToolError,
};
use leti_protocol::ErrorDto;
use serde_json::Value;

/// HTTP-shaped error. Routes return `Result<T, AppError>`; the
/// `IntoResponse` impl emits `Json<ErrorDto>` with the status.
#[derive(Debug)]
pub struct AppError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    pub details: Option<Value>,
}

impl AppError {
    #[must_use]
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            details: None,
        }
    }

    #[must_use]
    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }

    pub fn bad_request(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, message)
    }

    pub fn not_found(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, code, message)
    }

    pub fn conflict(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, code, message)
    }

    pub fn internal(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, code, message)
    }

    pub fn unsupported_media_type(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNSUPPORTED_MEDIA_TYPE, code, message)
    }

    pub fn unprocessable_entity(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, code, message)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status;
        let class = self.code;
        if status.is_server_error() {
            tracing::error!(class = class, status = status.as_u16(), message = %self.message, "request failed");
        } else if status.is_client_error() {
            tracing::warn!(class = class, status = status.as_u16(), message = %self.message, "request rejected");
        }
        let body = ErrorDto {
            code: self.code.to_string(),
            message: self.message,
            details: self.details,
        };
        (status, Json(body)).into_response()
    }
}

impl From<MemoryError> for AppError {
    fn from(e: MemoryError) -> Self {
        match e {
            MemoryError::SessionNotFound => Self::not_found("session_not_found", e.to_string()),
            MemoryError::MessageNotFound => Self::not_found("message_not_found", e.to_string()),
            MemoryError::Io(m) => Self::internal("memory_io", m),
            MemoryError::Unimplemented => Self::internal("memory_unimplemented", e.to_string()),
        }
    }
}

impl From<ArtifactError> for AppError {
    fn from(e: ArtifactError) -> Self {
        match e {
            ArtifactError::NotFound(p) => {
                Self::not_found("artifact_not_found", format!("artifact not found: {p}"))
            }
            ArtifactError::Io(m) => Self::internal("artifact_io", m),
            ArtifactError::Unimplemented => Self::internal("artifact_unimplemented", e.to_string()),
        }
    }
}

impl From<EventError> for AppError {
    fn from(e: EventError) -> Self {
        match e {
            EventError::BusClosed => Self::internal("event_bus_closed", e.to_string()),
            EventError::CursorTooFarBehind {
                requested,
                tip,
                window,
            } => Self::new(
                axum::http::StatusCode::CONFLICT,
                "cursor_too_far_behind",
                format!(
                    "Last-Event-ID {requested} is more than {window} rows behind tip {tip}; reconnect without Last-Event-ID"
                ),
            ),
            EventError::Io(m) => Self::internal("event_io", m),
            EventError::Unimplemented => Self::internal("event_unimplemented", e.to_string()),
        }
    }
}

impl From<PermissionError> for AppError {
    fn from(e: PermissionError) -> Self {
        match e {
            PermissionError::AskNotFound => Self::not_found("ask_not_found", e.to_string()),
            PermissionError::AskExpired => Self::not_found("ask_expired", e.to_string()),
            PermissionError::Timeout => Self::conflict("ask_timeout", e.to_string()),
            PermissionError::Unsupported(m) => Self::bad_request("unsupported_scope", m),
            PermissionError::Io(m) => Self::internal("permission_io", m),
            PermissionError::Unimplemented => {
                Self::internal("permission_unimplemented", e.to_string())
            }
        }
    }
}

impl From<ConfigError> for AppError {
    fn from(e: ConfigError) -> Self {
        match e {
            ConfigError::Invalid(m) => Self::bad_request("config_invalid", m),
            ConfigError::Io(m) => Self::internal("config_io", m),
        }
    }
}

impl From<ProviderError> for AppError {
    fn from(e: ProviderError) -> Self {
        // Provider response bodies may echo the request payload
        // (some upstreams do for 400s) including conversation context +
        // partially-substituted secrets. Log internally; return only a
        // fixed message to the client.
        match e {
            ProviderError::MissingCredentials { .. } | ProviderError::Auth(_) => {
                tracing::warn!(error = %e, "provider auth error");
                Self::new(
                    StatusCode::UNAUTHORIZED,
                    "provider_auth",
                    "upstream auth error",
                )
            }
            ProviderError::RateLimit { retry_after_ms } => Self::new(
                StatusCode::TOO_MANY_REQUESTS,
                "provider_rate_limit",
                format!("upstream rate limit (retry after {retry_after_ms}ms)"),
            ),
            ProviderError::Network(_) => {
                tracing::warn!(error = %e, "provider network error");
                Self::new(
                    StatusCode::BAD_GATEWAY,
                    "provider_network",
                    "upstream network error",
                )
            }
            ProviderError::Decode(_) => {
                tracing::warn!(error = %e, "provider decode error");
                Self::new(
                    StatusCode::BAD_GATEWAY,
                    "provider_decode",
                    "upstream decode error",
                )
            }
            ProviderError::ContextWindowExceeded { .. } => Self::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "context_window",
                "context window exceeded",
            ),
            ProviderError::Cancelled => {
                Self::conflict("provider_cancelled", "provider request cancelled")
            }
            ProviderError::Unimplemented => {
                Self::internal("provider_unimplemented", "provider not implemented")
            }
        }
    }
}

impl From<ToolError> for AppError {
    fn from(e: ToolError) -> Self {
        let class = e.class();
        Self::new(StatusCode::BAD_REQUEST, class.as_str(), e.to_string())
    }
}

impl From<FsError> for AppError {
    fn from(e: FsError) -> Self {
        match e {
            FsError::OutsideWorkspace(_) => {
                Self::bad_request("fs_outside_workspace", "path is outside the workspace")
            }
            FsError::NotFound(_) => Self::not_found("file_not_found", "file not found"),
            FsError::TooLarge { .. } => Self::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "fs_file_too_large",
                e.to_string(),
            ),
            FsError::Binary(_) => Self::unsupported_media_type("fs_binary", "binary file"),
            FsError::InvalidInput(m) => Self::bad_request("fs_invalid_input", m),
            FsError::Io(m) => Self::internal("fs_io", m),
            FsError::Unsupported(m) => Self::new(StatusCode::NOT_IMPLEMENTED, "fs_unsupported", m),
        }
    }
}

impl From<leti_core::error::CoreError> for AppError {
    fn from(e: leti_core::error::CoreError) -> Self {
        use leti_core::error::CoreError::*;
        match e {
            Provider(x) => x.into(),
            Memory(x) => x.into(),
            Artifact(x) => x.into(),
            Tool(x) => x.into(),
            Event(x) => x.into(),
            Permission(x) => x.into(),
            Config(x) => x.into(),
            ContextOverflowAfterCompaction => AppError::internal(
                "context_overflow_after_compaction",
                "context still overflows after compaction; manual conversation trim required",
            ),
        }
    }
}

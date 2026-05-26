//! Closed enum of failure classes used by telemetry + audit redaction.
//!
//! Extracted from `error.rs` so the taxonomy lives separately from the
//! `*Error` enums it classifies. Users never see the variant name; the
//! `as_str` mapping is the stable telemetry label.

/// Closed enum of failure classes. Telemetry layer maps each to a
/// `&'static str`; users never see the variant name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    Memory,
    Artifact,
    Event,
    Permission,
    Config,
    ProviderAuth,
    ProviderRateLimit,
    ProviderNetwork,
    ProviderDecode,
    ProviderCancelled,
    ProviderUnimplemented,
    ContextWindow,
    ToolPathOutsideWorkspace,
    ToolPermissionDenied,
    ToolReadBeforeWrite,
    ToolBinaryFile,
    ToolFileTooLarge,
    ToolNotFound,
    ToolInvalidInput,
    ToolTimeout,
    ToolIo,
    ToolNotAllowedInAgent,
    ToolUnimplemented,
}

impl FailureClass {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Artifact => "artifact",
            Self::Event => "event",
            Self::Permission => "permission",
            Self::Config => "config",
            Self::ProviderAuth => "provider_auth",
            Self::ProviderRateLimit => "provider_rate_limit",
            Self::ProviderNetwork => "provider_network",
            Self::ProviderDecode => "provider_decode",
            Self::ProviderCancelled => "provider_cancelled",
            Self::ProviderUnimplemented => "provider_unimplemented",
            Self::ContextWindow => "context_window",
            Self::ToolPathOutsideWorkspace => "tool_path_outside_workspace",
            Self::ToolPermissionDenied => "tool_permission_denied",
            Self::ToolReadBeforeWrite => "tool_read_before_write",
            Self::ToolBinaryFile => "tool_binary_file",
            Self::ToolFileTooLarge => "tool_file_too_large",
            Self::ToolNotFound => "tool_not_found",
            Self::ToolInvalidInput => "tool_invalid_input",
            Self::ToolTimeout => "tool_timeout",
            Self::ToolIo => "tool_io",
            Self::ToolNotAllowedInAgent => "tool_not_allowed_in_agent",
            Self::ToolUnimplemented => "tool_unimplemented",
        }
    }
}

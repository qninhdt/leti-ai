//! Error types for the six adapter traits + a top-level `CoreError`.
//!
//! No `Other(String)` variants. Where wrapping is needed
//! we use `class: &'static str` so failure-class taxonomy stays closed.

use thiserror::Error;

/// Top-level core error. Each variant `From`-wraps a subordinate enum.
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("provider error: {0}")]
    Provider(#[from] ProviderError),

    #[error("memory error: {0}")]
    Memory(#[from] MemoryError),

    #[error("artifact error: {0}")]
    Artifact(#[from] ArtifactError),

    #[error("tool error: {0}")]
    Tool(#[from] ToolError),

    #[error("event error: {0}")]
    Event(#[from] EventError),

    #[error("permission error: {0}")]
    Permission(#[from] PermissionError),

    #[error("config error: {0}")]
    Config(#[from] ConfigError),

    #[error("context still overflows after compaction summary; manual trim required")]
    ContextOverflowAfterCompaction,
}

impl CoreError {
    /// Closed-set failure class for telemetry + structured error responses.
    /// Adding a class requires editing this match — no free-form strings.
    #[must_use]
    pub fn class(&self) -> FailureClass {
        match self {
            Self::Provider(e) => e.class(),
            Self::Memory(_) => FailureClass::Memory,
            Self::Artifact(_) => FailureClass::Artifact,
            Self::Tool(e) => e.class(),
            Self::Event(_) => FailureClass::Event,
            Self::Permission(_) => FailureClass::Permission,
            Self::Config(_) => FailureClass::Config,
            Self::ContextOverflowAfterCompaction => FailureClass::ContextWindow,
        }
    }
}

/// Closed enum of failure classes — moved to a sibling submodule.
pub mod failure_class;
pub use failure_class::FailureClass;

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("missing credentials for {provider}: set {env_var}")]
    MissingCredentials {
        provider: &'static str,
        env_var: &'static str,
    },
    #[error("provider auth failed: {0}")]
    Auth(String),
    #[error("provider rate-limited; retry after {retry_after_ms}ms")]
    RateLimit { retry_after_ms: u64 },
    #[error("provider network error: {0}")]
    Network(String),
    #[error("provider response decode failed: {0}")]
    Decode(String),
    #[error("context window exceeded: {used} > {limit}")]
    ContextWindowExceeded { used: u64, limit: u64 },
    #[error("provider request cancelled")]
    Cancelled,
    #[error("not implemented")]
    Unimplemented,
}

impl ProviderError {
    #[must_use]
    pub fn class(&self) -> FailureClass {
        match self {
            Self::MissingCredentials { .. } | Self::Auth(_) => FailureClass::ProviderAuth,
            Self::RateLimit { .. } => FailureClass::ProviderRateLimit,
            Self::Network(_) => FailureClass::ProviderNetwork,
            Self::Decode(_) => FailureClass::ProviderDecode,
            Self::ContextWindowExceeded { .. } => FailureClass::ContextWindow,
            Self::Cancelled => FailureClass::ProviderCancelled,
            Self::Unimplemented => FailureClass::ProviderUnimplemented,
        }
    }
}

#[derive(Debug, Error)]
pub enum MemoryError {
    #[error("session not found")]
    SessionNotFound,
    #[error("message not found")]
    MessageNotFound,
    #[error("storage io: {0}")]
    Io(String),
    #[error("not implemented")]
    Unimplemented,
}

#[derive(Debug, Error)]
pub enum ArtifactError {
    #[error("artifact not found: {0}")]
    NotFound(String),
    #[error("artifact io: {0}")]
    Io(String),
    #[error("not implemented")]
    Unimplemented,
}

#[derive(Debug, Clone, Error)]
pub enum ToolError {
    #[error("path outside workspace: {0}")]
    PathOutsideWorkspace(String),
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error(
        "read before write required: file {0} must be read with the read tool before edit/write"
    )]
    ReadBeforeWriteRequired(String),
    #[error("binary file: {0}")]
    BinaryFile(String),
    #[error("file too large: {path} ({bytes} bytes > {limit} bytes)")]
    FileTooLarge {
        path: String,
        bytes: u64,
        limit: u64,
    },
    #[error("tool not found: {0}")]
    NotFound(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("tool execution timed out")]
    Timeout,
    #[error("tool io: {0}")]
    Io(String),
    /// Tool name not present in the active agent's `tool_allowlist`.
    /// Surfaced to the model as a corrected-error tool result so it can
    /// pivot to an allowed tool without seeing a permission failure.
    #[error("tool '{tool}' not allowed in agent '{agent}'")]
    NotAllowedInAgent { tool: String, agent: String },
    #[error("not implemented")]
    Unimplemented,
}

impl From<FsError> for ToolError {
    fn from(e: FsError) -> Self {
        match e {
            FsError::OutsideWorkspace(p) => ToolError::PathOutsideWorkspace(p),
            FsError::NotFound(p) => ToolError::Io(format!("not found: {p}")),
            FsError::TooLarge { path, bytes, limit } => {
                ToolError::FileTooLarge { path, bytes, limit }
            }
            FsError::Binary(p) => ToolError::BinaryFile(p),
            FsError::InvalidInput(m) => ToolError::InvalidInput(m),
            FsError::Io(m) => ToolError::Io(m),
        }
    }
}

/// Errors raised by `Filesystem` adapters. Mapped into `ToolError` at
/// the tool boundary via the `From` impl above. Adapter authors only
/// reach for this set; tools translate as needed.
#[derive(Debug, Error)]
pub enum FsError {
    #[error("path outside workspace: {0}")]
    OutsideWorkspace(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("file too large: {path} ({bytes} > {limit})")]
    TooLarge {
        path: String,
        bytes: u64,
        limit: u64,
    },
    #[error("binary file: {0}")]
    Binary(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("io: {0}")]
    Io(String),
}

impl ToolError {
    #[must_use]
    pub fn class(&self) -> FailureClass {
        match self {
            Self::PathOutsideWorkspace(_) => FailureClass::ToolPathOutsideWorkspace,
            Self::PermissionDenied(_) => FailureClass::ToolPermissionDenied,
            Self::ReadBeforeWriteRequired(_) => FailureClass::ToolReadBeforeWrite,
            Self::BinaryFile(_) => FailureClass::ToolBinaryFile,
            Self::FileTooLarge { .. } => FailureClass::ToolFileTooLarge,
            Self::NotFound(_) => FailureClass::ToolNotFound,
            Self::InvalidInput(_) => FailureClass::ToolInvalidInput,
            Self::Timeout => FailureClass::ToolTimeout,
            Self::Io(_) => FailureClass::ToolIo,
            Self::NotAllowedInAgent { .. } => FailureClass::ToolNotAllowedInAgent,
            Self::Unimplemented => FailureClass::ToolUnimplemented,
        }
    }
}

#[derive(Debug, Error)]
pub enum EventError {
    #[error("event bus closed")]
    BusClosed,
    #[error("cursor {requested} too far behind tip {tip} (window {window})")]
    CursorTooFarBehind {
        requested: i64,
        tip: i64,
        window: i64,
    },
    #[error("storage io: {0}")]
    Io(String),
    #[error("not implemented")]
    Unimplemented,
}

#[derive(Debug, Error)]
pub enum PermissionError {
    #[error("permission ask not found")]
    AskNotFound,
    #[error("permission ask expired")]
    AskExpired,
    #[error("permission ask timed out")]
    Timeout,
    #[error("scope not yet supported: {0}")]
    Unsupported(String),
    #[error("storage io: {0}")]
    Io(String),
    #[error("not implemented")]
    Unimplemented,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("invalid config: {0}")]
    Invalid(String),
    #[error("config io: {0}")]
    Io(String),
}

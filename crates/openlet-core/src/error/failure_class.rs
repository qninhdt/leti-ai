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

#[cfg(test)]
mod tests {
    use super::FailureClass;

    /// Table-driven mapping. Adding a `FailureClass` variant requires
    /// editing this table; the test forces a slug review per addition.
    const TABLE: &[(FailureClass, &str)] = &[
        (FailureClass::Memory, "memory"),
        (FailureClass::Artifact, "artifact"),
        (FailureClass::Event, "event"),
        (FailureClass::Permission, "permission"),
        (FailureClass::Config, "config"),
        (FailureClass::ProviderAuth, "provider_auth"),
        (FailureClass::ProviderRateLimit, "provider_rate_limit"),
        (FailureClass::ProviderNetwork, "provider_network"),
        (FailureClass::ProviderDecode, "provider_decode"),
        (FailureClass::ProviderCancelled, "provider_cancelled"),
        (
            FailureClass::ProviderUnimplemented,
            "provider_unimplemented",
        ),
        (FailureClass::ContextWindow, "context_window"),
        (
            FailureClass::ToolPathOutsideWorkspace,
            "tool_path_outside_workspace",
        ),
        (FailureClass::ToolPermissionDenied, "tool_permission_denied"),
        (FailureClass::ToolReadBeforeWrite, "tool_read_before_write"),
        (FailureClass::ToolBinaryFile, "tool_binary_file"),
        (FailureClass::ToolFileTooLarge, "tool_file_too_large"),
        (FailureClass::ToolNotFound, "tool_not_found"),
        (FailureClass::ToolInvalidInput, "tool_invalid_input"),
        (FailureClass::ToolTimeout, "tool_timeout"),
        (FailureClass::ToolIo, "tool_io"),
        (
            FailureClass::ToolNotAllowedInAgent,
            "tool_not_allowed_in_agent",
        ),
        (FailureClass::ToolUnimplemented, "tool_unimplemented"),
    ];

    #[test]
    fn every_variant_maps_to_its_telemetry_slug() {
        for (variant, expected) in TABLE {
            assert_eq!(
                variant.as_str(),
                *expected,
                "telemetry slug drift for {variant:?}"
            );
        }
    }

    #[test]
    fn slugs_are_lowercase_snake_case() {
        for (_variant, slug) in TABLE {
            assert!(
                slug.chars()
                    .all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit()),
                "slug {slug:?} must be lowercase snake_case"
            );
            assert!(!slug.is_empty(), "no empty slugs");
            assert!(!slug.starts_with('_'), "no leading underscore: {slug:?}");
            assert!(!slug.ends_with('_'), "no trailing underscore: {slug:?}");
        }
    }

    #[test]
    fn slugs_are_unique() {
        use std::collections::HashSet;
        let mut seen = HashSet::new();
        for (_variant, slug) in TABLE {
            assert!(seen.insert(*slug), "duplicate slug: {slug:?}");
        }
    }
}

//! Hyphen/slash-strict prefix matching for model names.
//!
//! Shared by the [`multi_provider`](crate::multi_provider) router and the
//! OpenAI-compat [`prefix_shaping`](crate::openai_compat::prefix_shaping)
//! quirk detector. A "strict" match requires the boundary between
//! `prefix` and the rest of the model name to be `-`, `/`, or
//! end-of-string — so `claude` does NOT match `claude2`, but
//! `claude-sonnet` matches both `claude` and `claude-`.
//!
//! Callers may pass `prefix` with or without a trailing separator;
//! a trailing `-` or `/` is treated as the boundary so existing
//! call-sites keyed on `"claude-"` / `"anthropic/"` keep working.

/// True when `model` starts with `prefix` AND the boundary after the
/// prefix is a hyphen, slash, or end-of-string. A trailing `-` or `/`
/// on `prefix` is stripped before the check so callers can encode the
/// separator inside the prefix or rely on the strict-boundary rule.
#[must_use]
pub(crate) fn strict_prefix(model: &str, prefix: &str) -> bool {
    let prefix = prefix.trim_end_matches(['-', '/']);
    if !model.starts_with(prefix) {
        return false;
    }
    let rest = &model[prefix.len()..];
    rest.is_empty() || rest.starts_with('-') || rest.starts_with('/')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_with_trailing_separator() {
        assert!(strict_prefix("claude-sonnet", "claude-"));
        assert!(strict_prefix("anthropic/foo", "anthropic/"));
        assert!(strict_prefix("claude-myprovider/foo", "claude-myprovider/"));
    }

    #[test]
    fn matches_without_trailing_separator() {
        assert!(strict_prefix("o1-mini", "o1"));
        assert!(strict_prefix("o1", "o1"));
        assert!(strict_prefix("kimi", "kimi"));
        assert!(strict_prefix("kimi-k2-0905", "kimi"));
    }

    #[test]
    fn rejects_non_separator_continuation() {
        // `claude2` must NOT match `claude` — boundary is not '-' or '/'.
        assert!(!strict_prefix("claude2", "claude"));
        assert!(!strict_prefix("o15", "o1"));
        assert!(!strict_prefix("o1xxx", "o1"));
    }
}

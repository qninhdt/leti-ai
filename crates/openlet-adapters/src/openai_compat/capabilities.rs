//! Per-model capability detection for the OpenAI-compat / OpenRouter
//! adapter.
//!
//! Match on a small allowlist of canonical model-name prefixes (NOT a
//! closed enum) so a new vision model from OpenAI / Anthropic / Google
//! works without an adapter release. The runtime queries this once per
//! turn before projection (`runtime::processor` rewrites unsupported
//! parts to text fallbacks).
//!
//! Keep this list deliberately narrow. False-positives (claiming
//! vision when the model lacks it) waste a turn on a 400 from the
//! provider; false-negatives (missing a vision-capable model) just
//! degrade gracefully to text.

use openlet_core::adapters::model_provider::ProviderCapabilities;

/// Resolves the per-model capability flags. Strips a leading
/// `provider/` prefix (`openai/gpt-4o` → `gpt-4o`) so callers can pass
/// either the bare model name or the OpenRouter slug.
#[must_use]
pub fn capabilities_for(model: &str) -> ProviderCapabilities {
    let bare = model.rsplit('/').next().unwrap_or(model);
    let lc = bare.to_ascii_lowercase();

    // Vision-capable model prefixes. Order doesn't matter — first
    // match wins. Each entry is a deliberate "we tested this" call;
    // when adding new ones, run a smoke turn against the provider
    // first to confirm the image_url block is accepted.
    let vision = matches_any(
        &lc,
        &[
            "gpt-4o",
            "gpt-5",
            "gpt-4-vision",
            "gpt-4-turbo",
            "claude-sonnet-",
            "claude-opus-",
            "claude-haiku-",
            "claude-3.5-sonnet",
            "claude-3.5-haiku",
            "claude-3-opus",
            "gemini-1.5-pro",
            "gemini-1.5-flash",
            "gemini-2.0-flash",
            "gemini-2.5-pro",
        ],
    );

    // Document-input (PDF block) capability — currently a tighter
    // subset than vision. Anthropic supports `document` blocks on
    // Claude 3.5+; OpenAI's API treats PDFs via the file-search tool,
    // not inline message content, so we project documents as text
    // there.
    let documents = matches_any(
        &lc,
        &[
            "claude-sonnet-",
            "claude-opus-",
            "claude-3.5-sonnet",
            "claude-3-opus",
        ],
    );

    ProviderCapabilities {
        supports_vision: vision,
        supports_document_input: documents,
        max_image_bytes: 0,
    }
}

fn matches_any(haystack: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|p| haystack.starts_with(p))
}

#[cfg(test)]
mod tests {
    use super::capabilities_for;

    #[test]
    fn known_vision_models_detected() {
        for m in [
            "gpt-4o",
            "gpt-4o-mini",
            "openai/gpt-4o",
            "anthropic/claude-sonnet-4-6",
            "claude-opus-4-1",
            "claude-haiku-4",
            "google/gemini-2.5-pro",
        ] {
            let caps = capabilities_for(m);
            assert!(caps.supports_vision, "expected vision support for {m}");
        }
    }

    #[test]
    fn unknown_models_default_to_no_capabilities() {
        let caps = capabilities_for("deepseek/deepseek-chat");
        assert!(!caps.supports_vision);
        assert!(!caps.supports_document_input);
        assert_eq!(caps.max_image_bytes, 0);
    }

    #[test]
    fn document_capability_narrower_than_vision() {
        // gpt-4o has vision but not inline document blocks.
        let caps = capabilities_for("openai/gpt-4o");
        assert!(caps.supports_vision);
        assert!(!caps.supports_document_input);
    }

    #[test]
    fn provider_prefix_stripped() {
        let caps = capabilities_for("openrouter-staging/gpt-4o");
        assert!(caps.supports_vision);
    }
}

//! Per-provider system-prompt overlay selection.
//!
//! Selects a static markdown overlay based on the model name. The
//! caller (`conversation::run_turn`) appends the overlay to the
//! caller-supplied `system_prompt` so each provider gets prompt text
//! tuned for its quirks (terseness, long-context, tool style).
//!
//! Selection is automatic and hyphen-strict: a custom OpenRouter
//! model named `claudemyfork` MUST NOT be routed to the Anthropic
//! overlay because it does not contain the family separator. Only
//! the canonical hyphenated forms (`claude-`, `gpt-`, `o1-`, `o3-`,
//! `gemini-`, `kimi-`, `moonshot-`) and the `anthropic/` slash form
//! match. Everything else falls through to the default overlay —
//! including future families like `o4-`, which the integrator can
//! map explicitly via the `OnChatMessages` plugin hook if needed.

const ANTHROPIC: &str = include_str!("../../assets/prompts/anthropic.md");
const GPT: &str = include_str!("../../assets/prompts/gpt.md");
const GEMINI: &str = include_str!("../../assets/prompts/gemini.md");
const KIMI: &str = include_str!("../../assets/prompts/kimi.md");
const DEFAULT: &str = include_str!("../../assets/prompts/default.md");

/// Returns the provider-specific system-prompt overlay for `model`.
///
/// Match order is fixed and tested below. The `anthropic/` slash
/// prefix matches OpenRouter-style routing keys; all other matches
/// require a trailing hyphen so partial-name collisions on custom
/// model identifiers fall through to [`DEFAULT`].
#[must_use]
pub fn select_provider_prompt(model: &str) -> &'static str {
    if model.starts_with("claude-") || model.starts_with("anthropic/") {
        ANTHROPIC
    } else if model.starts_with("gpt-") || model.starts_with("o1-") || model.starts_with("o3-") {
        GPT
    } else if model.starts_with("gemini-") {
        GEMINI
    } else if model.starts_with("kimi-") || model.starts_with("moonshot-") {
        KIMI
    } else {
        DEFAULT
    }
}

/// Composes a final system prompt by appending the provider overlay
/// to a caller-supplied base. When `base` is empty/None the overlay
/// is returned alone. The overlay is suffixed (not prefixed) so the
/// integrator's instructions take precedence in models that weight
/// the head of the system prompt more heavily.
#[must_use]
pub fn compose_system_prompt(base: Option<&str>, model: &str) -> String {
    let overlay = select_provider_prompt(model);
    match base {
        Some(b) if !b.is_empty() => format!("{b}\n\n{overlay}"),
        _ => overlay.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_hyphenated_routes_to_anthropic() {
        assert_eq!(select_provider_prompt("claude-sonnet-4-6"), ANTHROPIC);
    }

    #[test]
    fn anthropic_slash_routes_to_anthropic() {
        assert_eq!(select_provider_prompt("anthropic/claude-opus-5"), ANTHROPIC);
    }

    #[test]
    fn claude_without_hyphen_falls_through_to_default() {
        // F5.5 / F1.10: custom OpenRouter forks must not collide with
        // the Anthropic overlay just because they share a substring.
        assert_eq!(select_provider_prompt("claudemyfork"), DEFAULT);
    }

    #[test]
    fn claude_hyphen_with_slash_suffix_matches_anthropic() {
        // `claude-myprovider/foo` still starts with `claude-` and so
        // routes to Anthropic. The hyphen is the separator, not the slash.
        assert_eq!(select_provider_prompt("claude-myprovider/foo"), ANTHROPIC);
    }

    #[test]
    fn gpt_5_routes_to_gpt() {
        assert_eq!(select_provider_prompt("gpt-5-pro"), GPT);
    }

    #[test]
    fn o1_mini_routes_to_gpt() {
        assert_eq!(select_provider_prompt("o1-mini"), GPT);
    }

    #[test]
    fn o3_pro_routes_to_gpt() {
        assert_eq!(select_provider_prompt("o3-pro"), GPT);
    }

    #[test]
    fn gemini_routes_to_gemini() {
        assert_eq!(select_provider_prompt("gemini-1.5-pro"), GEMINI);
    }

    #[test]
    fn kimi_routes_to_kimi() {
        assert_eq!(select_provider_prompt("kimi-k2"), KIMI);
    }

    #[test]
    fn moonshot_routes_to_kimi() {
        assert_eq!(select_provider_prompt("moonshot-v1"), KIMI);
    }

    #[test]
    fn unknown_falls_through_to_default() {
        assert_eq!(select_provider_prompt("unknown-foo"), DEFAULT);
    }

    #[test]
    fn future_o_series_falls_through_to_default() {
        // F1.10: `o4-mini` is not in the o1/o3 prefix set today and
        // must fall through. Integrator can override via OnChatMessages.
        assert_eq!(select_provider_prompt("o4-mini"), DEFAULT);
    }

    #[test]
    fn overlays_differ_so_selection_is_observable() {
        // Sanity: the five overlays must not be identical, otherwise
        // routing tests above are vacuous.
        let overlays = [ANTHROPIC, GPT, GEMINI, KIMI, DEFAULT];
        for i in 0..overlays.len() {
            for j in (i + 1)..overlays.len() {
                assert_ne!(overlays[i], overlays[j], "overlays {i} and {j} must differ");
            }
        }
    }

    #[test]
    fn compose_returns_overlay_alone_when_base_empty() {
        assert_eq!(compose_system_prompt(None, "claude-sonnet-4-6"), ANTHROPIC);
        assert_eq!(
            compose_system_prompt(Some(""), "claude-sonnet-4-6"),
            ANTHROPIC
        );
    }

    #[test]
    fn compose_appends_overlay_to_base() {
        let base = "you are a helpful assistant";
        let composed = compose_system_prompt(Some(base), "gpt-5");
        assert!(composed.starts_with(base));
        assert!(composed.ends_with(GPT));
        assert!(composed.contains("\n\n"));
    }
}

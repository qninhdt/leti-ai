//! Per-provider system-prompt overlay selection, rendered via Tera.
//!
//! Every prompt string lives under `assets/prompts/` as a Tera template
//! embedded at compile time (`include_str!`), parsed once into a shared
//! [`Tera`] instance behind a [`LazyLock`]. A provider overlay is chosen
//! by model name and rendered; the caller (`conversation::run_turn`)
//! appends the overlay to the caller-supplied `system_prompt`.
//!
//! Selection is automatic and hyphen-strict: a custom OpenRouter model
//! named `claudemyfork` MUST NOT be routed to the Anthropic overlay
//! because it does not contain the family separator. Only the canonical
//! hyphenated forms (`claude-`, `gpt-`, `o1-`, `o3-`, `gemini-`,
//! `kimi-`, `moonshot-`) and the `anthropic/` slash form match.
//! Everything else falls through to the default overlay.
//!
//! The overlays share `base.md` via Tera inheritance (`{% extends %}`),
//! so common operating principles, verification steps, and safety rules
//! are authored once and each provider file overrides only its
//! `{% block provider %}`.

use std::sync::LazyLock;

use tera::{Context, Tera};

/// Template name for the summarization instruction used by compaction.
/// Rendered lazily via [`compaction_request`]; replaces the former
/// `COMPACTION_REQUEST` string constant.
const COMPACTION_TEMPLATE: &str = "ops/compaction.md";

/// Shared Tera instance holding every embedded prompt template. Parsed
/// once on first use. Templates are compiled into the binary via
/// `include_str!`, so there is zero runtime disk IO and the binary is
/// self-contained regardless of the working directory.
static PROMPTS: LazyLock<Tera> = LazyLock::new(|| {
    let mut tera = Tera::new();
    // Prompt text is Markdown, not HTML — disable autoescaping so `<`,
    // `&`, and `"` are never mangled into entities. `.md` is not in the
    // default escape suffix set, but we set this explicitly so a future
    // rename can't silently start escaping.
    tera.autoescape_on(Vec::<&str>::new());
    // Parents must load with their children; `add_raw_templates` inserts
    // the whole batch then finalizes inheritance, so order within the
    // slice does not matter. A malformed template or broken `{% extends %}`
    // is a build-time authoring error, hence the panic — it can never
    // depend on runtime input.
    tera.add_raw_templates([
        ("base.md", include_str!("../../assets/prompts/base.md")),
        (
            "anthropic.md",
            include_str!("../../assets/prompts/anthropic.md"),
        ),
        ("gpt.md", include_str!("../../assets/prompts/gpt.md")),
        ("gemini.md", include_str!("../../assets/prompts/gemini.md")),
        ("kimi.md", include_str!("../../assets/prompts/kimi.md")),
        (
            "default.md",
            include_str!("../../assets/prompts/default.md"),
        ),
        (
            COMPACTION_TEMPLATE,
            include_str!("../../assets/prompts/ops/compaction.md"),
        ),
    ])
    .expect("embedded prompt templates must parse and resolve inheritance");
    tera
});

/// Returns the template name of the provider overlay for `model`.
///
/// Match order is fixed and tested below. The `anthropic/` slash prefix
/// matches OpenRouter-style routing keys; all other matches require a
/// trailing hyphen so partial-name collisions on custom model
/// identifiers fall through to `default.md`.
fn overlay_template(model: &str) -> &'static str {
    if model.starts_with("claude-") || model.starts_with("anthropic/") {
        "anthropic.md"
    } else if model.starts_with("gpt-") || model.starts_with("o1-") || model.starts_with("o3-") {
        "gpt.md"
    } else if model.starts_with("gemini-") {
        "gemini.md"
    } else if model.starts_with("kimi-") || model.starts_with("moonshot-") {
        "kimi.md"
    } else {
        "default.md"
    }
}

/// Render the provider-specific system-prompt overlay for `model`.
///
/// The overlays take no template variables today, so rendering is
/// effectively infallible; a render error would mean an authoring bug in
/// an embedded template, which the [`PROMPTS`] build-time parse already
/// guards against. We still surface any error as a panic rather than
/// silently returning empty prompt text.
#[must_use]
pub fn select_provider_prompt(model: &str) -> String {
    PROMPTS
        .render(overlay_template(model), &Context::new())
        .expect("embedded prompt template must render")
        .trim()
        .to_string()
}

/// The summarization instruction asked of the model during compaction.
/// Sourced from `ops/compaction.md`. Phrased to preserve
/// goal/decisions/files while dropping tool-output bodies.
#[must_use]
pub fn compaction_request() -> String {
    PROMPTS
        .render(COMPACTION_TEMPLATE, &Context::new())
        .expect("embedded compaction template must render")
        .trim()
        .to_string()
}

/// Composes a final system prompt by appending the provider overlay to a
/// caller-supplied base. When `base` is empty/None the overlay is
/// returned alone. The overlay is suffixed (not prefixed) so the
/// integrator's instructions take precedence in models that weight the
/// head of the system prompt more heavily.
#[must_use]
pub fn compose_system_prompt(base: Option<&str>, model: &str) -> String {
    let overlay = select_provider_prompt(model);
    match base {
        Some(b) if !b.is_empty() => format!("{b}\n\n{overlay}"),
        _ => overlay,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_hyphenated_routes_to_anthropic() {
        assert_eq!(overlay_template("claude-sonnet-4-6"), "anthropic.md");
    }

    #[test]
    fn anthropic_slash_routes_to_anthropic() {
        assert_eq!(overlay_template("anthropic/claude-opus-5"), "anthropic.md");
    }

    #[test]
    fn claude_without_hyphen_falls_through_to_default() {
        // Custom OpenRouter forks must not collide with the Anthropic
        // overlay just because they share a substring.
        assert_eq!(overlay_template("claudemyfork"), "default.md");
    }

    #[test]
    fn claude_hyphen_with_slash_suffix_matches_anthropic() {
        // `claude-myprovider/foo` still starts with `claude-` and so
        // routes to Anthropic. The hyphen is the separator, not the slash.
        assert_eq!(overlay_template("claude-myprovider/foo"), "anthropic.md");
    }

    #[test]
    fn gpt_5_routes_to_gpt() {
        assert_eq!(overlay_template("gpt-5-pro"), "gpt.md");
    }

    #[test]
    fn o1_mini_routes_to_gpt() {
        assert_eq!(overlay_template("o1-mini"), "gpt.md");
    }

    #[test]
    fn o3_pro_routes_to_gpt() {
        assert_eq!(overlay_template("o3-pro"), "gpt.md");
    }

    #[test]
    fn gemini_routes_to_gemini() {
        assert_eq!(overlay_template("gemini-1.5-pro"), "gemini.md");
    }

    #[test]
    fn kimi_routes_to_kimi() {
        assert_eq!(overlay_template("kimi-k2"), "kimi.md");
    }

    #[test]
    fn moonshot_routes_to_kimi() {
        assert_eq!(overlay_template("moonshot-v1"), "kimi.md");
    }

    #[test]
    fn unknown_falls_through_to_default() {
        assert_eq!(overlay_template("unknown-foo"), "default.md");
    }

    #[test]
    fn future_o_series_falls_through_to_default() {
        // `o4-mini` is not in the o1/o3 prefix set today and must fall
        // through. Integrator can override via OnChatMessages.
        assert_eq!(overlay_template("o4-mini"), "default.md");
    }

    #[test]
    fn every_overlay_renders_non_empty() {
        for model in ["claude-x", "gpt-5", "gemini-2", "kimi-k2", "unknown"] {
            assert!(
                !select_provider_prompt(model).is_empty(),
                "overlay for {model} rendered empty"
            );
        }
    }

    #[test]
    fn overlays_share_base_but_differ_by_provider() {
        // The shared base content appears in every overlay (inheritance
        // works), while each provider block makes the output distinct.
        let anthropic = select_provider_prompt("claude-x");
        let gpt = select_provider_prompt("gpt-5");
        assert!(
            anthropic.contains("production-bound"),
            "base identity block must be inherited"
        );
        assert!(
            gpt.contains("production-bound"),
            "base identity block must be inherited"
        );
        assert_ne!(anthropic, gpt, "provider blocks must make overlays differ");
    }

    #[test]
    fn all_provider_overlays_are_distinct() {
        let overlays = [
            select_provider_prompt("claude-x"),
            select_provider_prompt("gpt-5"),
            select_provider_prompt("gemini-2"),
            select_provider_prompt("kimi-k2"),
            select_provider_prompt("unknown-foo"),
        ];
        for i in 0..overlays.len() {
            for j in (i + 1)..overlays.len() {
                assert_ne!(overlays[i], overlays[j], "overlays {i} and {j} must differ");
            }
        }
    }

    #[test]
    fn compose_returns_overlay_alone_when_base_empty() {
        assert_eq!(
            compose_system_prompt(None, "claude-sonnet-4-6"),
            select_provider_prompt("claude-sonnet-4-6")
        );
        assert_eq!(
            compose_system_prompt(Some(""), "claude-sonnet-4-6"),
            select_provider_prompt("claude-sonnet-4-6")
        );
    }

    #[test]
    fn compose_appends_overlay_to_base() {
        let base = "you are a helpful assistant";
        let composed = compose_system_prompt(Some(base), "gpt-5");
        assert!(composed.starts_with(base));
        assert!(composed.ends_with(&select_provider_prompt("gpt-5")));
        assert!(composed.contains("\n\n"));
    }

    #[test]
    fn compaction_request_renders_instruction() {
        let req = compaction_request();
        assert!(req.contains("Summarize the conversation"));
        assert!(!req.is_empty());
    }
}

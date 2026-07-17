//! `@subagent_name` mention parser — strict ASCII, prompt-anchored.
//!
//! Routes only when the FIRST character of the prompt is `@`. Mid-prompt
//! mentions are intentionally ignored: a user typing
//! "ask the @planner to..." should NOT silently trigger a subagent;
//! they have to start the line with `@planner`. Using `\w` would let
//! Unicode confusables (Cyrillic `а`, Greek `α`) match — so the slug class
//! is restricted to ASCII alphanumerics + `-` and `_`.

use crate::agent::{AgentRegistry, AgentSlug};
use regex::Regex;
use std::sync::OnceLock;

/// `^@SLUG\s+OBJECTIVE` — anchored, ASCII-only character class.
/// SLUG must start with a letter (matches `AgentSlug::new` 2..=64 length
/// constraint via the `{0,63}` quantifier on tail chars).
fn mention_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // No `\w` — that's Unicode-aware in `regex`. Explicit ASCII class.
        Regex::new(r"(?s)^@([a-zA-Z][a-zA-Z0-9_-]{0,63})\s+(.+)$").expect("static regex compiles")
    })
}

/// Parse a leading `@subagent` mention. Returns `(slug, objective)` if:
///  - The prompt starts with `@` (no leading whitespace)
///  - The slug matches the kebab-case `AgentSlug` validator
///  - The slug is registered in `registry`
///  - There is at least one whitespace + non-empty objective after
///
/// Returns `None` for any other input. Callers MUST NOT rewrite when
/// `None` — leaving the prompt literal is the safe default.
#[must_use]
pub fn parse_subagent_mention(
    prompt: &str,
    registry: &AgentRegistry,
) -> Option<(AgentSlug, String)> {
    let caps = mention_re().captures(prompt)?;
    let slug_str = caps.get(1)?.as_str();
    let objective = caps.get(2)?.as_str().trim();
    if objective.is_empty() {
        return None;
    }
    // The mention parser regex permits `_` and uppercase ASCII so model
    // output mentioning `@Worker_1` is at least visible. `AgentSlug::new`
    // is the source of truth for what's actually registrable, so a
    // mention that doesn't normalize to a valid slug fails resolution
    // here and the prompt stays literal.
    let slug = AgentSlug::new(slug_str.to_string()).ok()?;
    registry.get(&slug)?;
    Some((slug, objective.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{AgentDefinition, AgentRegistry, AgentSlug};

    fn registry_with(slugs: &[&str]) -> AgentRegistry {
        let mut r = AgentRegistry::new();
        for s in slugs {
            let slug = AgentSlug::new((*s).to_string()).expect("test slug");
            r.insert(AgentDefinition {
                slug: slug.clone(),
                title: s.to_string(),
                description: String::new(),
                prompt_segments: None,
                tool_allowlist: Vec::new(),
                model_id: Some("test-model".to_string()),
                default_temperature: 0.0,
                context_window: 128_000,
                compaction_threshold: 0.8,
                compaction_summary_cap_tokens: 2_048,
                hidden: false,
            })
            .expect("unique slug");
        }
        r
    }

    #[test]
    fn happy_path_admin() {
        let r = registry_with(&["admin"]);
        let out = parse_subagent_mention("@admin foo bar", &r);
        assert!(matches!(&out, Some((s, o)) if s.as_str() == "admin" && o == "foo bar"));
    }

    #[test]
    fn unregistered_slug_returns_none() {
        let r = registry_with(&["admin"]);
        assert!(parse_subagent_mention("@nonexistent foo", &r).is_none());
    }

    #[test]
    fn cyrillic_a_rejected() {
        // First char is U+0430 CYRILLIC SMALL LETTER A — not ASCII.
        let r = registry_with(&["admin"]);
        let cyr = "@\u{0430}dmin foo";
        assert!(parse_subagent_mention(cyr, &r).is_none());
    }

    #[test]
    fn mid_prompt_mention_rejected() {
        let r = registry_with(&["admin"]);
        assert!(parse_subagent_mention("\n@admin foo", &r).is_none());
        assert!(parse_subagent_mention(" @admin foo", &r).is_none());
        assert!(parse_subagent_mention("hi @admin foo", &r).is_none());
    }

    #[test]
    fn no_objective_rejected() {
        let r = registry_with(&["admin"]);
        assert!(parse_subagent_mention("@admin", &r).is_none());
        assert!(parse_subagent_mention("@admin\n", &r).is_none());
        assert!(parse_subagent_mention("@admin   ", &r).is_none());
    }

    #[test]
    fn email_like_path_does_not_match_dotted_slug() {
        // `.` is not in the slug character class — the leading mention
        // is `@example`, then space, then objective `please`.
        let r = registry_with(&["example"]);
        // Single token (no whitespace) → no objective; rejected.
        assert!(parse_subagent_mention("@example.com please", &r).is_none());
    }

    #[test]
    fn admin_with_at_in_objective() {
        let r = registry_with(&["admin"]);
        let out = parse_subagent_mention("@admin @example.com please review", &r);
        let (slug, obj) = out.expect("matches");
        assert_eq!(slug.as_str(), "admin");
        assert_eq!(obj, "@example.com please review");
    }

    #[test]
    fn multiline_objective_preserved() {
        let r = registry_with(&["admin"]);
        let out = parse_subagent_mention("@admin do this\nand that", &r);
        let (_, obj) = out.expect("matches");
        assert_eq!(obj, "do this\nand that");
    }
}

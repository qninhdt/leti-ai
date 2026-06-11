//! Evidence scrubber for real-LLM acceptance transcripts.
//!
//! Real-LLM acceptance runs capture full prompts + model responses to
//! gitignored `evidence/` files for audit/debug, and Phase 14 uploads
//! them off-box as CI artifacts. Two controls keep secrets + PII out of
//! those artifacts (M18). The primary control is **synthetic-only
//! fixtures** — acceptance prompts use fabricated names/emails, never real
//! PII, enforced by [`assert_synthetic_fixture`]. As defense in depth,
//! [`scrub_transcript`] runs a **redaction pass** removing credential-shaped
//! tokens and email addresses before any write.
//!
//! The API key itself is already safe at the provider layer
//! (`SecretString` with `set_sensitive(true)`), so the residual risk this
//! module addresses is conversational content that leaks into a transcript.

use std::sync::LazyLock;

use regex::Regex;

/// What a redacted span is replaced with — a stable marker so a reader
/// sees *that* something was removed without seeing the value.
const REDACTED: &str = "[REDACTED]";

/// Credential + PII patterns, most specific first. Each match is replaced
/// by [`REDACTED`]. Compiled once.
static PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        // OpenAI/OpenRouter-style secret keys: sk-... (incl. sk-or-...).
        r"sk-[A-Za-z0-9_-]{12,}",
        // Bearer tokens in an Authorization header value.
        r"(?i)bearer\s+[A-Za-z0-9._-]{8,}",
        // OPENROUTER_*/OPENAI_* style `KEY=value` env assignments.
        r"(?i)OPEN(?:ROUTER|AI)_[A-Z_]*\s*=\s*\S+",
        // Email addresses (conversational PII — the primary residual risk).
        r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}",
    ]
    .iter()
    .map(|p| Regex::new(p).expect("static scrubber regex compiles"))
    .collect()
});

/// Redact credential-shaped tokens and emails from a transcript before it
/// is written to the gitignored `evidence/` dir. Idempotent — running it
/// twice leaves the `[REDACTED]` markers untouched.
#[must_use]
pub fn scrub_transcript(input: &str) -> String {
    let mut out = input.to_string();
    for re in PATTERNS.iter() {
        out = re.replace_all(&out, REDACTED).into_owned();
    }
    out
}

/// Returns `Err` listing any substrings in `text` that look like real PII
/// a synthetic fixture must not contain. Callers assert `Ok(())` over each
/// acceptance prompt so the primary control (synthetic-only inputs) is
/// machine-checked, not just a convention.
pub fn assert_synthetic_fixture(text: &str) -> Result<(), String> {
    let scrubbed = scrub_transcript(text);
    if scrubbed != text {
        return Err(format!(
            "fixture contains credential/PII-shaped content that the scrubber \
             would redact; acceptance prompts must be synthetic. Offending input: {text:?}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planted_secret_and_email_are_removed() {
        // Plant a known fake secret + fake email, assert both are gone.
        let transcript = "user said: my key is sk-or-v1-abcdef0123456789 \
             and email alice@example.com please help";
        let scrubbed = scrub_transcript(transcript);
        assert!(
            !scrubbed.contains("sk-or-v1-abcdef0123456789"),
            "secret key must be redacted: {scrubbed}"
        );
        assert!(
            !scrubbed.contains("alice@example.com"),
            "email must be redacted: {scrubbed}"
        );
        assert!(scrubbed.contains(REDACTED));
    }

    #[test]
    fn bearer_and_env_assignment_are_removed() {
        let t = "Authorization: Bearer sk-test-tokenvalue123\nOPENROUTER_API_KEY=secretval";
        let s = scrub_transcript(t);
        assert!(!s.contains("sk-test-tokenvalue123"), "bearer token: {s}");
        assert!(!s.contains("secretval"), "env value: {s}");
    }

    #[test]
    fn scrub_is_idempotent() {
        let once = scrub_transcript("key sk-abcdefghijkl now");
        let twice = scrub_transcript(&once);
        assert_eq!(once, twice, "running the scrubber twice is a no-op");
    }

    #[test]
    fn clean_text_is_unchanged() {
        let clean = "The agent called the read tool and returned 3 files.";
        assert_eq!(scrub_transcript(clean), clean);
    }

    #[test]
    fn synthetic_fixture_guard_flags_real_looking_pii() {
        assert!(assert_synthetic_fixture("contact me at bob@corp.com").is_err());
        assert!(assert_synthetic_fixture("read the file and summarize").is_ok());
    }
}

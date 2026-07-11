//! Literal extraction from a regex pattern for the ReDoS-safe grep prefilter.
//!
//! The cloud grep is two-phase (see `mod.rs`): a Postgres trigram prefilter
//! over LITERAL substrings, then an in-process linear-time `regex` re-match.
//! This module owns phase-1 input: pulling literal substrings (>= 3 chars) out
//! of the caller's pattern so the backend never sees the regex itself.
//!
//! # Covering invariant (the whole point)
//!
//! The backend prefilter is `extracted_text ILIKE ANY(literals)` — a UNION. For
//! that union to be a safe SUPERSET of the regex's true match set, EVERY string
//! the regex matches must contain AT LEAST ONE of the returned literals. If it
//! does not, a real match is silently excluded — the exact parity failure this
//! feature exists to avoid. So the rule is: return a literal set only when we
//! can PROVE it covers every match; otherwise return empty (which the caller
//! treats as "no prefilter" and does a full in-process scan — always correct,
//! just less selective).
//!
//! # Why HIR, not a hand-rolled scanner
//!
//! A lexical scan over the pattern string cannot reason about structure. For
//! `fo|\d+` the `\d+` branch has no literal, so `["foo"]` is NOT covering (a
//! line `12345` matches but lacks `foo`). For `abc{123}` the `{123}` interior
//! is a repeat count, not text, so `"123"` is a false literal. For `\p{Greek}`
//! the `Greek` is a class name, not match text.
//!
//! We delegate to `regex_syntax`'s prefix-literal extractor, which operates on
//! the parsed HIR. It maintains exactly the covering invariant: when it cannot
//! enumerate a covering set (a literal-less alternation branch, an unbounded
//! interior, a large class) it marks the sequence inexact/infinite and
//! `literals()` returns `None`. We then fall back to the full-scan path. It can
//! never fabricate a literal from a quantifier interior or an escape body,
//! because it never looks at raw pattern characters.

use regex_syntax::ParserBuilder;
use regex_syntax::hir::literal::{Extractor, Seq};

/// Minimum literal length. Trigram (`pg_trgm`) indexes are keyed on 3-grams, so
/// a literal shorter than 3 chars cannot be index-accelerated. More importantly
/// for CORRECTNESS: if ANY literal in the covering set is below this floor we
/// must NOT drop just that one (that would break coverage — the branch it
/// covers would go unmatched). Instead we drop the WHOLE set and full-scan.
pub(crate) const MIN_LITERAL_LEN: usize = 3;

/// Extract index-usable literal substrings from `pattern`.
///
/// Returns a set of literals such that EVERY string matched by `pattern`
/// contains at least one of them (the covering invariant). Returns EMPTY when
/// no such all-≥3-char covering set can be proven — the caller then does a full
/// in-process scan. Never invents a literal a match could lack.
///
/// `case_insensitive` must match the grep's own flag: it changes which HIR is
/// produced (case-insensitive classes), and hence which literals are required.
/// The backend prefilter uses `ILIKE`, so case folding also happens there; we
/// pass the flag through so extraction sees the same pattern the re-match will.
pub(crate) fn extract_literals(pattern: &str, case_insensitive: bool) -> Vec<String> {
    let hir = match ParserBuilder::new()
        .case_insensitive(case_insensitive)
        .build()
        .parse(pattern)
    {
        Ok(h) => h,
        // Unparseable pattern: the re-match will reject it identically, so the
        // literal set is irrelevant. Empty = full-scan (the caller's compile
        // step surfaces the real error first anyway).
        Err(_) => return Vec::new(),
    };

    // Prefix extraction: the returned Seq, when finite+exact-or-inexact, is a
    // set of literals each of which is a required prefix-anchored fragment such
    // that every match starts with one of them. `literals()` returns None when
    // the sequence is infinite (not covering) — our full-scan signal.
    let seq: Seq = Extractor::new().extract(&hir);

    let Some(lits) = seq.literals() else {
        // Infinite / not-covering (e.g. a `.*`-ish or literal-less branch).
        return Vec::new();
    };
    if lits.is_empty() {
        return Vec::new();
    }

    // COVERING CHECK: every literal must clear the trigram floor. If even one is
    // shorter than MIN_LITERAL_LEN, the union of the rest is not covering (the
    // branch that short literal represents would be excluded from the
    // prefilter). Drop the entire set → full-scan. This is the correctness
    // guard that makes a partial/short extraction safe.
    let mut out: Vec<String> = Vec::with_capacity(lits.len());
    for lit in lits {
        // A non-exact literal is only a PREFIX of the required fragment (the
        // extractor truncated it); it is still required, so it still counts for
        // coverage as long as it clears the floor. CAUTION: prefix literals are
        // BYTE prefixes and codepoint-unaware — an inexact one can be truncated
        // mid-UTF-8-codepoint. Lossy-decoding would turn the dangling bytes into
        // U+FFFD and, if the result still clears the floor, push a literal
        // (`abc\u{FFFD}`) that does NOT occur in the real text — silently
        // excluding a genuine match, the exact covering violation this module
        // guards against. So require valid UTF-8: a non-UTF-8 byte prefix is
        // un-provable coverage → drop the WHOLE set and full-scan.
        let Ok(s) = std::str::from_utf8(lit.as_bytes()) else {
            return Vec::new();
        };
        if s.chars().count() < MIN_LITERAL_LEN {
            return Vec::new();
        }
        out.push(s.to_string());
    }

    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extract(p: &str) -> Vec<String> {
        extract_literals(p, false)
    }

    #[test]
    fn plain_literal_is_extracted() {
        assert_eq!(extract("TODO"), vec!["TODO".to_string()]);
    }

    #[test]
    fn short_literal_below_trigram_floor_forces_full_scan() {
        // `ab` (2 chars) cannot be trigram-accelerated → empty (full-scan).
        assert!(extract("ab").is_empty());
        assert_eq!(extract("abc"), vec!["abc".to_string()]);
    }

    #[test]
    fn alternation_all_branches_have_literal_is_covering() {
        // Both branches contribute a >=3 literal → covering union.
        let mut got = extract("error|warning");
        got.sort();
        assert_eq!(got, vec!["error".to_string(), "warning".to_string()]);
    }

    #[test]
    fn alternation_with_literalless_branch_forces_full_scan() {
        // REGRESSION (reviewer C1): `foo|\d+` — the `\d+` branch has no
        // literal, so `["foo"]` would WRONGLY exclude a line matching only
        // `\d+`. Must fall back to full-scan (empty).
        assert!(extract(r"foo|\d+").is_empty());
        // `error|.*` — the `.*` branch matches anything; not coverable.
        assert!(extract("error|.*").is_empty());
        // Short branch drags the whole set below the floor.
        assert!(extract("foo|ab").is_empty());
    }

    #[test]
    fn bounded_quantifier_interior_is_not_a_literal() {
        // REGRESSION (reviewer H1): `abc{123}` matches `ab` + 123 `c`s; the
        // `{123}` is a repeat count, NOT text. `"123"` must never appear as a
        // literal. The guaranteed prefix here is `abc` (a,b, then >=1 c).
        let got = extract("abc{123}");
        assert!(
            !got.iter().any(|l| l.contains("123")),
            "quantifier interior leaked as literal: {got:?}"
        );
    }

    #[test]
    fn multichar_escape_body_is_not_a_literal() {
        // REGRESSION (reviewer H2): `\p{Greek}abc` — `Greek` is a class name.
        // A match is a Greek codepoint + `abc`; `"Greek"` must not be a literal.
        // `abc` is NOT a guaranteed PREFIX (the Greek char precedes it), so
        // prefix extraction yields the Greek class (infinite/large) → full-scan.
        let got = extract(r"\p{Greek}abc");
        assert!(
            !got.iter().any(|l| l.contains("Greek")),
            "escape body leaked as literal: {got:?}"
        );
    }

    #[test]
    fn wildcard_prefix_extracts_leading_literal() {
        // `TODO.*urgent` — every match starts with `TODO`; that alone is a
        // covering prefix set. (The re-match still enforces the full pattern.)
        assert_eq!(extract("TODO.*urgent"), vec!["TODO".to_string()]);
    }

    #[test]
    fn redos_pattern_extracts_no_dangerous_literal() {
        // The classic catastrophic-backtracking patterns have no covering
        // literal >= 3, so the prefilter is a bounded ready-set scan — nothing
        // pathological ever reaches Postgres, and the linear re-match can't nest.
        assert!(extract("(a+)+$").is_empty());
        assert!(extract("(.*a){20}").is_empty());
    }

    #[test]
    fn optional_char_enumerates_covering_alternation() {
        // `colou?r` — the `u` is optional. The extractor enumerates BOTH exact
        // spellings `{color, colour}`; that set is covering (every match equals
        // one of them) and each clears the floor. This is stronger than a bare
        // common prefix and still correct.
        let mut got = extract("colou?r");
        got.sort();
        assert_eq!(got, vec!["color".to_string(), "colour".to_string()]);
        // Every literal returned must be one the two real match strings contain.
        for lit in &got {
            assert!(
                "color" == lit.as_str() || "colour" == lit.as_str(),
                "literal {lit:?} is not a real match string"
            );
        }
    }

    #[test]
    fn every_returned_literal_is_valid_utf8_and_present_in_a_match() {
        // Guard against the inexact-byte-prefix / phantom-U+FFFD covering
        // violation: for a pattern mixing ASCII with a multibyte codepoint, any
        // literal we return must be valid UTF-8 AND actually occur in a real
        // matching string (never a synthesized replacement char).
        for p in [r"abc\w*文", "abc文def", r"foo.*文字", "café", "naïve.*x"] {
            for lit in extract(p) {
                assert!(
                    !lit.contains('\u{FFFD}'),
                    "pattern {p:?} produced a phantom replacement-char literal {lit:?}"
                );
                assert!(lit.chars().count() >= MIN_LITERAL_LEN);
            }
        }
    }

    #[test]
    fn case_insensitive_literal_still_covers() {
        // With the case-insensitive flag the HIR folds case; the literal set
        // (if any) must still be covering. `abc` under (?i) may expand to a
        // large alternation and fall back to full-scan — either way, never a
        // WRONG non-empty set.
        let got = extract_literals("abcdef", true);
        for lit in &got {
            assert!(lit.chars().count() >= MIN_LITERAL_LEN);
        }
    }
}

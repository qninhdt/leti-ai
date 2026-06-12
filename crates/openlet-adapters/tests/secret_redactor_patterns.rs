//! Pattern coverage for [`SecretRedactor`] — the credential scrubber that
//! guards JSONL audit logs from leaking secrets.
//!
//! Two redaction mechanisms, both tested here:
//!   1. Token-prefix regex denylist (9 patterns) applied to STRING VALUES.
//!   2. Whole-name match on sensitive JSON KEYS (api_key, authorization, …).
//!
//! The table test drives one secret per regex pattern through
//! `redact_in_place` and asserts the secret no longer appears in the output.
//! Boundary cases pin the false-positive guards: nested JSON, multi-line
//! input, and look-alikes that must NOT be redacted.

use openlet_adapters::localfs::SecretRedactor;
use serde_json::{Value, json};

/// Redact a bare string and return the scrubbed form.
fn redact_str(redactor: &SecretRedactor, input: &str) -> String {
    let mut v = Value::String(input.to_string());
    redactor.redact_in_place(&mut v);
    v.as_str().expect("string in, string out").to_string()
}

#[test]
fn each_pattern_redacts_its_secret() {
    let r = SecretRedactor::default();

    // (label, input containing a secret, the secret substring that must vanish)
    let cases: &[(&str, &str, &str)] = &[
        (
            "bearer token",
            "Authorization: Bearer abc123DEF456_ghi.jkl=",
            "abc123DEF456_ghi.jkl=",
        ),
        (
            "openai sk- key",
            "key is sk-abcdefghijklmnop0123456789",
            "sk-abcdefghijklmnop0123456789",
        ),
        (
            "stripe live key",
            "charge with sk_live_abcdefghijklmnop1234",
            "sk_live_abcdefghijklmnop1234",
        ),
        (
            "aws access key",
            "aws id AKIAIOSFODNN7EXAMPLE here",
            "AKIAIOSFODNN7EXAMPLE",
        ),
        (
            "gcp api key",
            "AIzaSyA1234567890abcdefghijklmnopqrstuvw is gcp",
            "AIzaSyA1234567890abcdefghijklmnopqrstuvw",
        ),
        (
            "github pat",
            "token ghp_0123456789abcdefghijklmnopqrstuvwxyz",
            "ghp_0123456789abcdefghijklmnopqrstuvwxyz",
        ),
        (
            "github oauth",
            "oauth gho_0123456789abcdefghijklmnopqrstuvwxyz",
            "gho_0123456789abcdefghijklmnopqrstuvwxyz",
        ),
        (
            "slack token",
            "slack xoxb-1234567890-abcdefghijkl token",
            "xoxb-1234567890-abcdefghijkl",
        ),
        (
            "jwt",
            "jwt eyJhbGciOiUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NQ.SflKxwRJSMeKKF2QT4",
            "eyJhbGciOiUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NQ.SflKxwRJSMeKKF2QT4",
        ),
    ];

    for (label, input, secret) in cases {
        let out = redact_str(&r, input);
        assert!(
            !out.contains(secret),
            "[{label}] secret leaked through redaction: input={input:?} output={out:?}"
        );
        assert!(
            out.contains("<redacted>"),
            "[{label}] expected a <redacted> marker: output={out:?}"
        );
    }
}

#[test]
fn redacts_secret_embedded_in_nested_json() {
    let r = SecretRedactor::default();
    // Secret buried inside a nested object's string value (not a sensitive
    // KEY — proves the value-regex path recurses into nested structures).
    let mut v = json!({
        "outer": {
            "log_line": "called provider with sk-ABCDEFGHIJKLMNOP0123456789 today",
            "nested": ["plain text", "AKIAIOSFODNN7EXAMPLE in an array element"]
        }
    });
    r.redact_in_place(&mut v);

    let serialized = v.to_string();
    assert!(
        !serialized.contains("sk-ABCDEFGHIJKLMNOP0123456789"),
        "nested object value not redacted: {serialized}"
    );
    assert!(
        !serialized.contains("AKIAIOSFODNN7EXAMPLE"),
        "array element value not redacted: {serialized}"
    );
}

#[test]
fn redacts_sensitive_keys_by_whole_name() {
    let r = SecretRedactor::default();
    // The VALUE of a sensitive key is replaced wholesale, regardless of
    // whether it matches a token-prefix pattern.
    let mut v = json!({
        "api_key": "not-a-recognized-prefix-but-still-secret",
        "authorization": "Basic dXNlcjpwYXNz",
        "password": "hunter2",
        "nested": { "token": "opaque-session-id" }
    });
    r.redact_in_place(&mut v);

    assert_eq!(v["api_key"], json!("<redacted>"));
    assert_eq!(v["authorization"], json!("<redacted>"));
    assert_eq!(v["password"], json!("<redacted>"));
    assert_eq!(v["nested"]["token"], json!("<redacted>"));
}

#[test]
fn does_not_redact_lookalike_key_names() {
    let r = SecretRedactor::default();
    // Whole-name match means `tokenizer` / `api_keyboard` must survive —
    // substring matching here would scrub legitimate fields.
    let mut v = json!({
        "tokenizer": "gpt2-bpe",
        "api_keyboard": "qwerty layout",
        "secretary": "Jane Doe"
    });
    r.redact_in_place(&mut v);

    assert_eq!(v["tokenizer"], json!("gpt2-bpe"));
    assert_eq!(v["api_keyboard"], json!("qwerty layout"));
    assert_eq!(v["secretary"], json!("Jane Doe"));
}

#[test]
fn redacts_secret_in_multiline_string() {
    let r = SecretRedactor::default();
    // A multi-line log blob with a secret on the middle line. The regex must
    // find it regardless of surrounding newlines.
    let input = "line one: nothing here\n\
                 line two: sk-ABCDEFGHIJKLMNOP0123456789 leaked\n\
                 line three: also nothing";
    let out = redact_str(&r, input);
    assert!(
        !out.contains("sk-ABCDEFGHIJKLMNOP0123456789"),
        "multi-line secret not redacted: {out}"
    );
    // Surrounding clean lines survive.
    assert!(out.contains("line one"));
    assert!(out.contains("line three"));
}

#[test]
fn leaves_clean_text_untouched() {
    let r = SecretRedactor::default();
    // No secret, no sensitive key → byte-for-byte identical output. Guards
    // against an over-eager pattern redacting ordinary prose.
    let input = "The quick brown fox wrote sketch notes about a tokenizer.";
    let out = redact_str(&r, input);
    assert_eq!(out, input, "clean text must pass through unchanged");
}

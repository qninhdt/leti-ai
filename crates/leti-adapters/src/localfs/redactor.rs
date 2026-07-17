//! `SecretRedactor` — regex-based credential scrubbing for JSONL audit logs.
//!
//! Detects common credential prefixes (OpenAI keys, AWS access keys, JWTs, etc.)
//! in string values and whole-name-matches sensitive JSON keys (e.g. `api_key`,
//! `authorization`). Designed to be shared across any serialization boundary
//! that might leak secrets into persistent storage.

use regex::Regex;
use serde_json::Value;

const SENSITIVE_KEYS: &[&str] = &[
    "api_key",
    "apikey",
    "authorization",
    "x-api-key",
    "password",
    "secret",
    "token",
    "access_token",
    "refresh_token",
];

#[derive(Debug)]
pub struct SecretRedactor {
    patterns: Vec<Regex>,
    sensitive: Vec<String>,
}

impl Default for SecretRedactor {
    fn default() -> Self {
        // Token-prefix denylist. Each pattern matches the
        // common form of a credential a model could exfiltrate via tool
        // output. Whole-name match for sensitive keys (no substring) so
        // legitimate names like `tokenizer` aren't false-positively
        // redacted.
        let raw_patterns = [
            r"(?i)bearer\s+[A-Za-z0-9\-_.=]+",
            r"sk-[A-Za-z0-9_\-]{16,}",     // OpenAI / Anthropic
            r"sk_live_[A-Za-z0-9]{16,}",   // Stripe
            r"AKIA[0-9A-Z]{16}",           // AWS
            r"AIza[0-9A-Za-z_\-]{35}",     // GCP
            r"gh[ps]_[A-Za-z0-9]{36}",     // GitHub PAT/server
            r"gho_[A-Za-z0-9]{36}",        // GitHub OAuth
            r"xox[abp]-[A-Za-z0-9-]{10,}", // Slack
            r"eyJ[A-Za-z0-9_\-]{20,}\.[A-Za-z0-9_\-]+\.[A-Za-z0-9_\-]+", // JWT
        ];
        let patterns = raw_patterns
            .iter()
            .map(|p| Regex::new(p).expect("redactor regex"))
            .collect();
        Self {
            patterns,
            sensitive: SENSITIVE_KEYS.iter().map(|s| s.to_lowercase()).collect(),
        }
    }
}

impl SecretRedactor {
    fn is_sensitive_key(&self, k: &str) -> bool {
        // Whole-name match (case-insensitive) so `tokenizer` doesn't
        // false-positively trigger on `token`.
        let lk = k.to_lowercase();
        self.sensitive.contains(&lk)
    }

    pub fn redact_in_place(&self, v: &mut Value) {
        match v {
            Value::Object(map) => {
                for (k, val) in map.iter_mut() {
                    if self.is_sensitive_key(k) {
                        *val = Value::String("<redacted>".into());
                    } else {
                        self.redact_in_place(val);
                    }
                }
            }
            Value::Array(arr) => {
                for item in arr.iter_mut() {
                    self.redact_in_place(item);
                }
            }
            Value::String(s) => {
                let mut redacted: std::borrow::Cow<'_, str> = std::borrow::Cow::Borrowed(s);
                for re in &self.patterns {
                    let next = re.replace_all(&redacted, "<redacted>");
                    redacted = std::borrow::Cow::Owned(next.into_owned());
                }
                *v = Value::String(redacted.into_owned());
            }
            _ => {}
        }
    }
}

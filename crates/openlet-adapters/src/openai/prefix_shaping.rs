//! Per-provider request shaping for the OpenAI-compat dialect.
//!
//! Real upstreams diverge from "vanilla" OpenAI Chat Completions in
//! small but lethal ways:
//!  - `gpt-5*` rejects `max_tokens` and requires `max_completion_tokens`
//!  - `o1-*` / `o3-*` and `grok-3-mini` reject sampling params
//!    (`temperature`, `top_p`, `frequency_penalty`, `presence_penalty`)
//!  - Moonshot Kimi rejects `is_error` on tool result messages
//!  - DashScope caps request body at 6 MiB; OpenAI at 100 MiB
//!
//! This module is a pure-function layer: it takes the model name + the
//! serialized request body (`serde_json::Value`) and rewrites in place.
//! Detection is hyphen/slash-strict — see [`detect_quirks`] — so a
//! custom OpenRouter model named `claude-myprovider/foo` does NOT
//! trigger Anthropic-flavored shaping.

use openlet_core::adapters::model_provider::ProviderCapabilities;
use openlet_core::error::ProviderError;
use serde_json::Value;

/// Hyphen/slash-strict prefix detection. Each pattern is followed by a
/// hyphen so `claude-` never matches `claude2` and `gpt-` never matches
/// `gptastic`. Slash-form (`anthropic/`) covers OpenRouter's
/// vendor-prefixed names. Returns the [`ProviderCapabilities`] flags
/// for that model family.
#[must_use]
pub fn detect_quirks(model: &str) -> ProviderCapabilities {
    let mut caps = ProviderCapabilities::default();
    if matches_prefix(model, "gpt-5") {
        caps.max_completion_tokens_param = true;
        caps.max_request_body_bytes = 100 * 1024 * 1024;
    } else if matches_prefix(model, "o1") || matches_prefix(model, "o3") {
        caps.strip_sampling_params = true;
        caps.max_request_body_bytes = 100 * 1024 * 1024;
    } else if matches_prefix(model, "grok-3-mini") {
        // grok-3-mini specifically — wider grok-3 / grok-4 keep params.
        caps.strip_sampling_params = true;
    } else if matches_prefix(model, "kimi") {
        caps.reject_is_error_field = true;
    } else if matches_prefix(model, "qwen") {
        // DashScope (Qwen) caps body at 6 MiB.
        caps.max_request_body_bytes = 6 * 1024 * 1024;
    }
    caps
}

/// True if `model` starts with `prefix` AND is followed by a `-` or
/// is exactly the prefix. Strict so `claude-myprovider/foo` doesn't
/// look like `claude` (it has the prefix but the next char is also
/// part of a custom name; collision case is handled by the
/// `MultiProvider` router via `prefix_overrides`).
fn matches_prefix(model: &str, prefix: &str) -> bool {
    if !model.starts_with(prefix) {
        return false;
    }
    let rest = &model[prefix.len()..];
    rest.is_empty() || rest.starts_with('-')
}

/// Apply quirk-driven mutations to the OpenAI-compat request body in
/// place. Caller passes the JSON it's about to serialize; we drop
/// fields, rename `max_tokens` → `max_completion_tokens`, etc. Returns
/// `Err(BodyTooLarge)` if the post-mutation body exceeds
/// `caps.max_request_body_bytes` (pre-flight cap; closes the failure
/// mode where DashScope returns 413 with no useful error).
pub fn apply_request_shaping(
    body: &mut Value,
    caps: ProviderCapabilities,
) -> Result<(), ProviderError> {
    let Some(obj) = body.as_object_mut() else {
        return Ok(());
    };

    if caps.max_completion_tokens_param {
        if let Some(v) = obj.remove("max_tokens") {
            obj.insert("max_completion_tokens".to_string(), v);
        }
    }

    if caps.strip_sampling_params {
        for k in &[
            "temperature",
            "top_p",
            "frequency_penalty",
            "presence_penalty",
        ] {
            obj.remove(*k);
        }
    }

    if caps.reject_is_error_field {
        if let Some(messages) = obj.get_mut("messages").and_then(Value::as_array_mut) {
            for msg in messages {
                if let Some(m) = msg.as_object_mut() {
                    m.remove("is_error");
                }
            }
        }
    }

    if caps.max_request_body_bytes > 0 {
        let serialized = serde_json::to_vec(body)
            .map_err(|e| ProviderError::Network(format!("body encode: {e}")))?;
        if serialized.len() > caps.max_request_body_bytes {
            return Err(ProviderError::Network(format!(
                "request body {} bytes exceeds provider cap {} bytes",
                serialized.len(),
                caps.max_request_body_bytes
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn o1_mini_strips_sampling_params() {
        let caps = detect_quirks("o1-mini");
        assert!(caps.strip_sampling_params);
        let mut body = json!({
            "model": "o1-mini",
            "temperature": 0.7,
            "top_p": 0.9,
            "frequency_penalty": 0.1,
            "presence_penalty": 0.0,
            "messages": []
        });
        apply_request_shaping(&mut body, caps).unwrap();
        let obj = body.as_object().unwrap();
        assert!(!obj.contains_key("temperature"));
        assert!(!obj.contains_key("top_p"));
        assert!(!obj.contains_key("frequency_penalty"));
        assert!(!obj.contains_key("presence_penalty"));
    }

    #[test]
    fn gpt5_renames_max_tokens() {
        let caps = detect_quirks("gpt-5-pro");
        assert!(caps.max_completion_tokens_param);
        let mut body = json!({
            "model": "gpt-5-pro",
            "max_tokens": 4000,
            "messages": []
        });
        apply_request_shaping(&mut body, caps).unwrap();
        let obj = body.as_object().unwrap();
        assert!(!obj.contains_key("max_tokens"));
        assert_eq!(obj["max_completion_tokens"], 4000);
    }

    #[test]
    fn kimi_drops_is_error_in_tool_result() {
        let caps = detect_quirks("kimi-k2-0905");
        assert!(caps.reject_is_error_field);
        let mut body = json!({
            "model": "kimi-k2-0905",
            "messages": [
                {"role": "tool", "content": "result", "is_error": true},
                {"role": "user", "content": "ok"}
            ]
        });
        apply_request_shaping(&mut body, caps).unwrap();
        let messages = body["messages"].as_array().unwrap();
        let m0 = messages[0].as_object().unwrap();
        assert!(!m0.contains_key("is_error"));
    }

    #[test]
    fn grok_3_mini_strips_sampling_params() {
        let caps = detect_quirks("grok-3-mini");
        assert!(caps.strip_sampling_params);
        let mut body = json!({"model": "grok-3-mini", "temperature": 0.7, "messages": []});
        apply_request_shaping(&mut body, caps).unwrap();
        assert!(!body.as_object().unwrap().contains_key("temperature"));
    }

    #[test]
    fn grok_3_full_keeps_sampling_params() {
        // Wider grok-3 (without -mini suffix) still accepts sampling
        // params. Strict-prefix check guards this.
        let caps = detect_quirks("grok-3");
        assert!(!caps.strip_sampling_params);
    }

    #[test]
    fn custom_openrouter_model_passes_through() {
        // Custom OpenRouter model `claude-myprovider/foo` must NOT
        // match Anthropic shaping — Anthropic detection lives in
        // MultiProvider, not here. detect_quirks returns defaults
        // because none of our quirk prefixes match.
        let caps = detect_quirks("claude-myprovider/foo");
        assert!(!caps.max_completion_tokens_param);
        assert!(!caps.strip_sampling_params);
        assert!(!caps.reject_is_error_field);
        assert_eq!(caps.max_request_body_bytes, 0);
    }

    #[test]
    fn dashscope_qwen_body_cap_enforced() {
        let caps = detect_quirks("qwen-max");
        assert_eq!(caps.max_request_body_bytes, 6 * 1024 * 1024);
        // Build a body that's just over the cap.
        let big = "x".repeat(7 * 1024 * 1024);
        let mut body = json!({"messages": [{"role": "user", "content": big}]});
        let err = apply_request_shaping(&mut body, caps).unwrap_err();
        match err {
            ProviderError::Network(s) => assert!(s.contains("exceeds provider cap")),
            other => panic!("expected Network, got {other:?}"),
        }
    }

    #[test]
    fn matches_prefix_strictness() {
        assert!(matches_prefix("o1-mini", "o1"));
        assert!(matches_prefix("o1", "o1"));
        // Hyphen required; "o15" must not match "o1".
        assert!(!matches_prefix("o15", "o1"));
        assert!(!matches_prefix("o1xxx", "o1"));
        // Empty rest with prefix-only name is fine.
        assert!(matches_prefix("kimi", "kimi"));
    }
}

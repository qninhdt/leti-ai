//! Phase 4 — `prefix_shaping::apply_request_shaping` cap edges + adversarial inputs.
//!
//! Inline tests in source already cover happy paths for each provider
//! family. This file exercises edges:
//!
//! 1. DashScope (`qwen-*`) at cap-1, cap, cap+1 byte boundaries.
//! 2. `strip_sampling_params` removes NaN-valued temperature cleanly
//!    (still removed; serializer doesn't get a chance to choke).
//! 3. Kimi `reject_is_error_field` strips across many messages.
//! 4. `gpt-5*` `max_tokens: null` rename produces
//!    `max_completion_tokens: null`.

use openlet_adapters::openai_compat::prefix_shaping::{apply_request_shaping, detect_quirks};
use openlet_core::error::ProviderError;
use serde_json::{Value, json};

fn body_size_bytes(v: &Value) -> usize {
    serde_json::to_vec(v).unwrap().len()
}

#[test]
fn dashscope_body_cap_passes_at_or_under_and_fails_above() {
    let caps = detect_quirks("qwen-max");
    let cap = caps.max_request_body_bytes;
    assert!(cap > 0);

    // Build a body whose serialized size sits exactly at `cap`. We
    // start with a small skeleton and pad the user content string
    // until the serialized body matches.
    fn body_with_pad(pad_len: usize) -> Value {
        json!({
            "messages": [{"role": "user", "content": "x".repeat(pad_len)}]
        })
    }

    // Find the pad that makes the body exactly `cap` bytes.
    let skeleton = body_with_pad(0);
    let overhead = body_size_bytes(&skeleton); // cost of `{"messages":[{"role":"user","content":""}]}`
    let pad_for_exact = cap - overhead;
    let mut at_cap = body_with_pad(pad_for_exact);
    assert_eq!(body_size_bytes(&at_cap), cap, "padding math wrong");

    // exact cap → ok (the check is `> max`, not `>=`)
    apply_request_shaping(&mut at_cap, caps).expect("at-cap body must pass");

    // cap-1 → ok
    let mut under = body_with_pad(pad_for_exact - 1);
    apply_request_shaping(&mut under, caps).expect("cap-1 body must pass");

    // cap+1 → fails with cap message
    let mut over = body_with_pad(pad_for_exact + 1);
    let err = apply_request_shaping(&mut over, caps).unwrap_err();
    match err {
        ProviderError::Network(s) => {
            assert!(s.contains("exceeds provider cap"), "{s}");
        }
        other => panic!("expected Network; got {other:?}"),
    }
}

#[test]
fn strip_sampling_params_removes_temperature_even_when_value_is_unusual() {
    // `f64::NaN` cannot be represented in JSON, so we can't actually
    // serialize it directly. Use a string sentinel instead — the
    // strip path doesn't care about the value, only the key.
    let caps = detect_quirks("o1-mini");
    let mut body = json!({
        "model": "o1-mini",
        "temperature": "irrelevant_value_will_be_stripped",
        "messages": []
    });
    apply_request_shaping(&mut body, caps).unwrap();
    assert!(!body.as_object().unwrap().contains_key("temperature"));
}

#[test]
fn kimi_strips_is_error_across_many_messages() {
    let caps = detect_quirks("kimi-k2-0905");
    let mut messages: Vec<Value> = Vec::with_capacity(100);
    for i in 0..100 {
        messages.push(json!({
            "role": "tool",
            "content": format!("result-{i}"),
            "is_error": true
        }));
    }
    let mut body = json!({"model": "kimi-k2-0905", "messages": messages});
    apply_request_shaping(&mut body, caps).unwrap();
    let arr = body["messages"].as_array().unwrap();
    assert_eq!(arr.len(), 100);
    for m in arr {
        assert!(
            !m.as_object().unwrap().contains_key("is_error"),
            "is_error must be stripped from every message"
        );
    }
}

#[test]
fn gpt5_renames_null_max_tokens_to_max_completion_tokens_null() {
    let caps = detect_quirks("gpt-5-pro");
    let mut body = json!({
        "model": "gpt-5-pro",
        "max_tokens": null,
        "messages": []
    });
    apply_request_shaping(&mut body, caps).unwrap();
    let obj = body.as_object().unwrap();
    assert!(!obj.contains_key("max_tokens"));
    assert!(obj.contains_key("max_completion_tokens"));
    assert!(obj["max_completion_tokens"].is_null());
}

//! Tests for the lossy `From<Usage>` → `UsageDto` conversion.
//!
//! The wire DTO intentionally merges `cache_write_tokens` and
//! `cache_creation_input_tokens` into a single field and drops `cost_usd`.

use openlet_core::types::event::Usage;
use openlet_protocol::UsageDto;

#[test]
fn normal_conversion_preserves_token_counts() {
    let usage = Usage {
        input_tokens: 100,
        output_tokens: 50,
        cached_input_tokens: 20,
        cache_write_tokens: 10,
        cache_creation_input_tokens: 5,
        reasoning_tokens: 30,
        cost_usd: None,
    };

    let dto = UsageDto::from(usage);

    assert_eq!(dto.input_tokens, 100);
    assert_eq!(dto.output_tokens, 50);
    assert_eq!(dto.cached_input_tokens, 20);
    assert_eq!(dto.reasoning_tokens, 30);
}

#[test]
fn cache_write_merges_both_fields() {
    let usage = Usage {
        input_tokens: 0,
        output_tokens: 0,
        cached_input_tokens: 0,
        cache_write_tokens: 42,
        cache_creation_input_tokens: 18,
        reasoning_tokens: 0,
        cost_usd: None,
    };

    let dto = UsageDto::from(usage);

    // Wire field is the sum of both domain fields.
    assert_eq!(dto.cache_write_tokens, 60);
}

#[test]
fn cache_write_saturates_at_u64_max() {
    let usage = Usage {
        input_tokens: 0,
        output_tokens: 0,
        cached_input_tokens: 0,
        cache_write_tokens: u64::MAX,
        cache_creation_input_tokens: 1,
        reasoning_tokens: 0,
        cost_usd: None,
    };

    let dto = UsageDto::from(usage);

    // Saturating add caps at u64::MAX instead of wrapping/panicking.
    assert_eq!(dto.cache_write_tokens, u64::MAX);
}

#[test]
fn cost_usd_is_not_surfaced_on_dto() {
    // `UsageDto` has no `cost_usd` field — it surfaces via
    // `StepFinished.cost_decimal_str` instead. This test verifies
    // the DTO struct doesn't carry the field by checking serde output.
    use std::str::FromStr;

    let usage = Usage {
        input_tokens: 1,
        output_tokens: 2,
        cached_input_tokens: 0,
        cache_write_tokens: 0,
        cache_creation_input_tokens: 0,
        reasoning_tokens: 0,
        cost_usd: Some(rust_decimal::Decimal::from_str("0.0042").unwrap()),
    };

    let dto = UsageDto::from(usage);
    let json = serde_json::to_value(&dto).unwrap();

    // The serialized DTO must not contain a cost field.
    assert!(json.get("cost_usd").is_none());
    // Token fields are still present.
    assert_eq!(json["input_tokens"], 1);
    assert_eq!(json["output_tokens"], 2);
}

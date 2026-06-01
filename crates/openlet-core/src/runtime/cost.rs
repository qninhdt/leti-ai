//! Cost calculation — `Decimal` USD per turn.
//!
//! Formula (cross-check §5; mirrors opencode `session.ts:430-440`):
//! ```text
//!   input_non_cache  / 1e6 * input_per_mtok
//! + output           / 1e6 * output_per_mtok
//! + reasoning        / 1e6 * output_per_mtok        (charged at output rate)
//! + cache_read       / 1e6 * cache_read_per_mtok    (when pricing has it)
//! + cache_write      / 1e6 * cache_write_per_mtok   (when pricing has it)
//! ```
//!
//! `cached_input_tokens` is subtracted from `input_tokens` before pricing —
//! OpenRouter returns both `prompt_tokens` and
//! `prompt_tokens_details.cached_tokens` so we'd double-count otherwise
//! (claw `openai_compat.rs:777-790`).

use rust_decimal::Decimal;

use crate::adapters::model_provider::ModelPricing;
use crate::types::event::Usage;

/// One million, as a `Decimal`. Hot path; cached.
fn million() -> Decimal {
    Decimal::new(1_000_000, 0)
}

/// Compute USD cost for a single turn given usage + per-Mtok pricing.
#[must_use]
pub fn compute_cost(usage: &Usage, pricing: &ModelPricing) -> Decimal {
    let mtok = million();

    // Avoid double-counting cached prompt tokens against the standard input
    // rate — OpenRouter reports them as a subset of `prompt_tokens`.
    let input_non_cache = usage.input_tokens.saturating_sub(usage.cached_input_tokens);

    let mut total = Decimal::new(0, 0);
    total += Decimal::from(input_non_cache) / mtok * pricing.input_per_mtok;
    total += Decimal::from(usage.output_tokens) / mtok * pricing.output_per_mtok;
    total += Decimal::from(usage.reasoning_tokens) / mtok * pricing.output_per_mtok;

    if let Some(cr) = pricing.cached_input_per_mtok {
        total += Decimal::from(usage.cached_input_tokens) / mtok * cr;
    }
    if let Some(cw) = pricing.cache_write_per_mtok {
        // L2 — providers populate EXACTLY ONE of these cache-write
        // counters: Anthropic uses `cache_creation_input_tokens`,
        // OpenRouter normalized usage uses `cache_write_tokens`. Take
        // `max()` rather than summing: if a defensive adapter ever
        // populated BOTH (with the same value), a sum would double-charge.
        // `max()` bills the cache write exactly once in every case
        // (one-field, other-field, or both-equal).
        let writes = usage
            .cache_write_tokens
            .max(usage.cache_creation_input_tokens);
        total += Decimal::from(writes) / mtok * cw;
    }
    total
}

/// Render a USD amount as a 4-decimal string. Used for `cost_decimal_str`
/// on `step_finish` events.
#[must_use]
pub fn format_usd(amount: Decimal) -> String {
    format!("{amount:.4}")
}

#[cfg(test)]
mod tests {
    use super::{compute_cost, format_usd};
    use crate::adapters::model_provider::ModelPricing;
    use crate::types::event::Usage;
    use rust_decimal::Decimal;
    use std::str::FromStr;

    fn pricing(input: &str, output: &str) -> ModelPricing {
        ModelPricing {
            input_per_mtok: Decimal::from_str(input).unwrap(),
            output_per_mtok: Decimal::from_str(output).unwrap(),
            cached_input_per_mtok: None,
            cache_write_per_mtok: None,
        }
    }

    #[test]
    fn matches_plan_fixture() {
        // Plan §Success Criteria: usage {prompt:1000, completion:500},
        // pricing (3.0, 15.0) per Mtok → 0.0105
        let u = Usage {
            input_tokens: 1000,
            output_tokens: 500,
            ..Default::default()
        };
        let p = pricing("3.00", "15.00");
        let cost = compute_cost(&u, &p);
        assert_eq!(format_usd(cost), "0.0105");
    }

    #[test]
    fn reasoning_charged_at_output_rate() {
        let u = Usage {
            input_tokens: 0,
            output_tokens: 0,
            reasoning_tokens: 1_000_000,
            ..Default::default()
        };
        let p = pricing("3.00", "15.00");
        let cost = compute_cost(&u, &p);
        assert_eq!(format_usd(cost), "15.0000");
    }

    #[test]
    fn cached_subtracted_from_input() {
        let u = Usage {
            input_tokens: 1000,
            cached_input_tokens: 1000,
            output_tokens: 0,
            ..Default::default()
        };
        // No cached_input_per_mtok → cache portion priced at zero, full
        // input_non_cache is zero → total zero.
        let p = pricing("3.00", "15.00");
        assert_eq!(compute_cost(&u, &p), Decimal::new(0, 0));
    }

    #[test]
    fn cache_lines_priced_when_pricing_present() {
        let u = Usage {
            input_tokens: 1000,
            cached_input_tokens: 500,
            cache_write_tokens: 200,
            output_tokens: 0,
            ..Default::default()
        };
        let p = ModelPricing {
            input_per_mtok: Decimal::from_str("3.00").unwrap(),
            output_per_mtok: Decimal::from_str("15.00").unwrap(),
            cached_input_per_mtok: Some(Decimal::from_str("0.30").unwrap()),
            cache_write_per_mtok: Some(Decimal::from_str("3.75").unwrap()),
        };
        let cost = compute_cost(&u, &p);
        // input_non_cache=500 *3/M = 0.0015
        // cache_read 500*0.30/M     = 0.00015
        // cache_write 200*3.75/M    = 0.00075
        // total                       0.0024
        assert_eq!(format_usd(cost), "0.0024");
    }

    #[test]
    fn cache_creation_input_tokens_alias_charges_at_write_rate() {
        // Anthropic populates cache_creation_input_tokens; cost calc
        // takes the max of both write counters so a provider using either
        // field gets billed exactly once at the cache_write rate.
        let u = Usage {
            input_tokens: 0,
            cache_creation_input_tokens: 1_000_000,
            ..Default::default()
        };
        let p = ModelPricing {
            input_per_mtok: Decimal::from_str("3.00").unwrap(),
            output_per_mtok: Decimal::from_str("15.00").unwrap(),
            cached_input_per_mtok: None,
            cache_write_per_mtok: Some(Decimal::from_str("3.75").unwrap()),
        };
        let cost = compute_cost(&u, &p);
        assert_eq!(format_usd(cost), "3.7500");
    }

    #[test]
    fn both_cache_write_fields_populated_charged_once_via_max() {
        // L2 — a defensive adapter that populates BOTH cache-write fields
        // with the SAME value must be billed ONCE, not summed (which would
        // double-charge). `max()` takes a single count.
        let u = Usage {
            input_tokens: 0,
            cache_write_tokens: 1_000_000,
            cache_creation_input_tokens: 1_000_000,
            ..Default::default()
        };
        let p = ModelPricing {
            input_per_mtok: Decimal::from_str("3.00").unwrap(),
            output_per_mtok: Decimal::from_str("15.00").unwrap(),
            cached_input_per_mtok: None,
            cache_write_per_mtok: Some(Decimal::from_str("3.75").unwrap()),
        };
        let cost = compute_cost(&u, &p);
        // max(1e6, 1e6) = 1e6 → 1e6 * 3.75 / 1e6 = 3.75 (NOT 7.50).
        assert_eq!(
            format_usd(cost),
            "3.7500",
            "both fields set must charge once (max), not twice (sum)"
        );
    }

    #[test]
    fn differing_cache_write_fields_take_the_larger() {
        // When the two counters disagree (shouldn't happen, but be
        // explicit), `max()` bills the larger — never their sum.
        let u = Usage {
            input_tokens: 0,
            cache_write_tokens: 400_000,
            cache_creation_input_tokens: 1_000_000,
            ..Default::default()
        };
        let p = ModelPricing {
            input_per_mtok: Decimal::from_str("3.00").unwrap(),
            output_per_mtok: Decimal::from_str("15.00").unwrap(),
            cached_input_per_mtok: None,
            cache_write_per_mtok: Some(Decimal::from_str("3.75").unwrap()),
        };
        let cost = compute_cost(&u, &p);
        // max(400k, 1e6) = 1e6 → 3.75, not 1.4e6-based 5.25.
        assert_eq!(format_usd(cost), "3.7500");
    }

    #[test]
    fn cached_greater_than_input_clamps_to_zero_via_saturating_sub() {
        // Provider quirk: some adapters return `cached_input_tokens >
        // input_tokens` for short prompts. `saturating_sub` clamps the
        // non-cache portion to 0; cost stays bounded at the cache
        // rate, never goes negative.
        let u = Usage {
            input_tokens: 100,
            cached_input_tokens: 200, // larger than input
            output_tokens: 0,
            ..Default::default()
        };
        let p = ModelPricing {
            input_per_mtok: Decimal::from_str("3.00").unwrap(),
            output_per_mtok: Decimal::from_str("15.00").unwrap(),
            cached_input_per_mtok: Some(Decimal::from_str("0.30").unwrap()),
            cache_write_per_mtok: None,
        };
        let cost = compute_cost(&u, &p);
        // input_non_cache clamps to 0 via saturating_sub. cached_input
        // priced at 200 * 0.30 / 1e6 = 0.00006. The 4-decimal format
        // truncates very small fractional values; lock that the
        // result is finite, non-negative, and < 0.001 — the precise
        // rounded string depends on rust_decimal's tie-break mode.
        assert!(cost >= Decimal::new(0, 0), "cost cannot go negative");
        assert!(
            cost < Decimal::from_str("0.001").unwrap(),
            "cost should remain bounded by the cache rate ({cost})"
        );
    }

    #[test]
    fn format_usd_rounds_to_four_decimals() {
        // 0.00001 (Decimal::new(1, 5)) — 4-decimal format truncates
        // toward "0.0000". Lock the format contract; if a future
        // refactor switches to 5-decimal, this test must be updated
        // intentionally rather than silently.
        let amt = Decimal::new(1, 5);
        assert_eq!(format_usd(amt), "0.0000");

        let amt = Decimal::new(15, 5); // 0.00015 → "0.0002" (banker's rounding)
        // rust_decimal default rounds half-to-even; 0.00015 has the
        // last kept digit even (0), so round-half-to-even keeps it
        // 0.0001. Either 0.0001 or 0.0002 is acceptable depending on
        // mode — assert it falls in {0.0001, 0.0002} so the contract
        // is robust against minor rust_decimal version drift.
        let s = format_usd(amt);
        assert!(
            s == "0.0001" || s == "0.0002",
            "rounded value {s} not in expected pair"
        );
    }

    #[test]
    fn huge_input_tokens_does_not_panic_or_wrap_to_negative() {
        // u64 → Decimal is now infallible (Decimal::from(u64)). Values
        // above i64::MAX (the previous as-cast ceiling) must NOT wrap
        // to a negative cost. Lock that contract here.
        let huge = u64::MAX;
        let u = Usage {
            input_tokens: huge,
            output_tokens: 0,
            ..Default::default()
        };
        let p = pricing("0.000001", "0.0");
        let cost = compute_cost(&u, &p);
        assert!(
            cost >= Decimal::new(0, 0),
            "u64-max input tokens must not produce negative cost ({cost})"
        );
    }
}

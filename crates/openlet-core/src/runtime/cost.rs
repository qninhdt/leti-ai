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
    total += Decimal::new(input_non_cache as i64, 0) / mtok * pricing.input_per_mtok;
    total += Decimal::new(usage.output_tokens as i64, 0) / mtok * pricing.output_per_mtok;
    total += Decimal::new(usage.reasoning_tokens as i64, 0) / mtok * pricing.output_per_mtok;

    if let Some(cr) = pricing.cached_input_per_mtok {
        total += Decimal::new(usage.cached_input_tokens as i64, 0) / mtok * cr;
    }
    if let Some(cw) = pricing.cache_write_per_mtok {
        total += Decimal::new(usage.cache_write_tokens as i64, 0) / mtok * cw;
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
}

//! Property-based invariants on `compute_cost`.
//!
//! Cost arithmetic must be monotonic, non-negative, and respect the
//! cache discount. Drift here would let billing reports drift away
//! from upstream provider invoices.

use leti_core::adapters::model_provider::ModelPricing;
use leti_core::runtime::cost::compute_cost;
use leti_core::types::event::Usage;
use proptest::prelude::*;
use rust_decimal::Decimal;

fn arb_decimal_price() -> impl Strategy<Value = Decimal> {
    // 0.0 .. 1000.0 USD per Mtok, 4-decimal granularity. Covers all
    // public price tables we've seen (gpt-4o, claude-opus, etc.).
    (0i64..10_000_000).prop_map(|v| Decimal::new(v, 4))
}

fn arb_pricing() -> impl Strategy<Value = ModelPricing> {
    (
        arb_decimal_price(),
        arb_decimal_price(),
        prop::option::of(arb_decimal_price()),
        prop::option::of(arb_decimal_price()),
    )
        .prop_map(|(input, output, cached, write)| ModelPricing {
            input_per_mtok: input,
            output_per_mtok: output,
            cached_input_per_mtok: cached,
            cache_write_per_mtok: write,
        })
}

fn arb_usage() -> impl Strategy<Value = Usage> {
    // Bound to realistic per-turn token counts. Avoids overflow on
    // intermediate i64 casts inside compute_cost (which the source
    // pins to ≤ i64::MAX explicitly).
    (
        0u64..1_000_000,
        0u64..1_000_000,
        0u64..1_000_000,
        0u64..1_000_000,
        0u64..1_000_000,
        0u64..1_000_000,
    )
        .prop_map(
            |(input, output, reasoning, cached, write, cache_create)| Usage {
                input_tokens: input,
                output_tokens: output,
                reasoning_tokens: reasoning,
                cached_input_tokens: cached,
                cache_write_tokens: write,
                cache_creation_input_tokens: cache_create,
                cost_usd: None,
            },
        )
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, .. ProptestConfig::default() })]

    /// Non-negative invariant: cost is never negative for any usage +
    /// pricing combination. Locks `saturating_sub` cache subtraction
    /// (cached > input must clamp to zero, not wrap to MAX).
    #[test]
    fn cost_is_non_negative(usage in arb_usage(), pricing in arb_pricing()) {
        let cost = compute_cost(&usage, &pricing);
        prop_assert!(cost >= Decimal::ZERO, "cost was negative: {}", cost);
    }

    /// Zero usage with any pricing → zero cost.
    #[test]
    fn zero_usage_yields_zero_cost(pricing in arb_pricing()) {
        let zero = Usage::default();
        prop_assert_eq!(compute_cost(&zero, &pricing), Decimal::ZERO);
    }

    /// Cumulative cost across N turns is monotonic non-decreasing.
    /// Wraps the per-turn non-negative invariant up to the session
    /// level: `Σ cost_per_turn` only ever grows.
    #[test]
    fn cumulative_cost_is_monotonic(
        turns in prop::collection::vec((arb_usage(), arb_pricing()), 0..20),
    ) {
        let mut total = Decimal::ZERO;
        for (u, p) in &turns {
            let next = total + compute_cost(u, p);
            prop_assert!(next >= total, "cumulative shrank from {} to {}", total, next);
            total = next;
        }
    }

    /// Cache discount: when `cached_input_per_mtok` is `Some(x)` AND
    /// strictly less than `input_per_mtok`, moving tokens from
    /// `input_tokens` (uncached) to the cached counter only ever
    /// decreases cost. If the discount is not honoured, billing leaks.
    #[test]
    fn cache_discount_lowers_cost_or_holds(
        non_cache in 100u64..100_000,
        cache in 0u64..100_000,
        input_rate_cents in 100i64..100_000,
        discount_cents in 0i64..100,
    ) {
        // Force discount strictly less than input rate.
        let input_per_mtok = Decimal::new(input_rate_cents, 4);
        let cached_per_mtok = Decimal::new(discount_cents, 4); // tiny

        let pricing = ModelPricing {
            input_per_mtok,
            output_per_mtok: Decimal::ZERO,
            cached_input_per_mtok: Some(cached_per_mtok),
            cache_write_per_mtok: None,
        };

        // Variant A: all tokens charged at non-cache rate.
        let all_uncached = Usage {
            input_tokens: non_cache + cache,
            cached_input_tokens: 0,
            ..Default::default()
        };
        // Variant B: `cache` tokens are cached.
        let with_cache = Usage {
            input_tokens: non_cache + cache,
            cached_input_tokens: cache,
            ..Default::default()
        };

        let cost_a = compute_cost(&all_uncached, &pricing);
        let cost_b = compute_cost(&with_cache, &pricing);
        prop_assert!(
            cost_b <= cost_a,
            "cache made it more expensive: a={} b={}",
            cost_a,
            cost_b,
        );
    }

    /// Reasoning tokens charged at OUTPUT rate. Locks the contract:
    /// `cost(input=0, output=0, reasoning=N)` equals `cost(input=0,
    /// output=N, reasoning=0)` for any positive N, given identical
    /// pricing.
    #[test]
    fn reasoning_priced_at_output_rate(
        n in 0u64..1_000_000,
        output_per_mtok in arb_decimal_price(),
    ) {
        let pricing = ModelPricing {
            input_per_mtok: Decimal::ZERO,
            output_per_mtok,
            cached_input_per_mtok: None,
            cache_write_per_mtok: None,
        };

        let as_output = Usage {
            output_tokens: n,
            ..Default::default()
        };
        let as_reasoning = Usage {
            reasoning_tokens: n,
            ..Default::default()
        };

        prop_assert_eq!(
            compute_cost(&as_output, &pricing),
            compute_cost(&as_reasoning, &pricing),
        );
    }

    /// `cache_write_tokens` and `cache_creation_input_tokens` are
    /// summed against the same write-rate. A provider populating
    /// either field gets billed identically.
    #[test]
    fn cache_write_aliases_charge_equivalently(n in 0u64..1_000_000) {
        let pricing = ModelPricing {
            input_per_mtok: Decimal::ZERO,
            output_per_mtok: Decimal::ZERO,
            cached_input_per_mtok: None,
            cache_write_per_mtok: Some(Decimal::new(37500, 4)), // 3.75 / Mtok
        };

        let via_write = Usage {
            cache_write_tokens: n,
            ..Default::default()
        };
        let via_create = Usage {
            cache_creation_input_tokens: n,
            ..Default::default()
        };

        prop_assert_eq!(
            compute_cost(&via_write, &pricing),
            compute_cost(&via_create, &pricing),
        );
    }
}

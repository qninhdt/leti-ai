//! Static OpenRouter pricing table.
//!
//! Decimal USD per million tokens. Update procedure:
//! 1. Pull from <https://openrouter.ai/models> manually.
//! 2. Increment a comment line above each entry on change.
//! 3. Bump `pricing_table_version()` when adding/removing rows.
//!
//! A config file would invite stale values nobody owns. A manual PR is the
//! cheapest authoritative path for an MVP-scale pricing surface.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::OnceLock;

use rust_decimal::Decimal;

use openlet_core::adapters::model_provider::ModelPricing;

/// Returns pricing for `model` if known. Match keys against the canonical
/// OpenRouter slug — callers MUST pass that exact form.
#[must_use]
pub fn pricing_for(model: &str) -> Option<ModelPricing> {
    table().get(model).cloned()
}

#[must_use]
pub fn pricing_table_version() -> &'static str {
    "2026-05-23.1"
}

fn table() -> &'static HashMap<&'static str, ModelPricing> {
    static TABLE: OnceLock<HashMap<&'static str, ModelPricing>> = OnceLock::new();
    TABLE.get_or_init(build_table)
}

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).expect("static pricing decimal")
}

fn build_table() -> HashMap<&'static str, ModelPricing> {
    let mut m = HashMap::new();

    // Anthropic via OpenRouter
    m.insert(
        "anthropic/claude-sonnet-4-6",
        ModelPricing {
            input_per_mtok: dec("3.00"),
            output_per_mtok: dec("15.00"),
            cached_input_per_mtok: Some(dec("0.30")),
            cache_write_per_mtok: Some(dec("3.75")),
        },
    );
    m.insert(
        "anthropic/claude-3.5-sonnet",
        ModelPricing {
            input_per_mtok: dec("3.00"),
            output_per_mtok: dec("15.00"),
            cached_input_per_mtok: Some(dec("0.30")),
            cache_write_per_mtok: Some(dec("3.75")),
        },
    );
    m.insert(
        "anthropic/claude-3.5-haiku",
        ModelPricing {
            input_per_mtok: dec("0.80"),
            output_per_mtok: dec("4.00"),
            cached_input_per_mtok: Some(dec("0.08")),
            cache_write_per_mtok: Some(dec("1.00")),
        },
    );

    // OpenAI via OpenRouter
    m.insert(
        "openai/gpt-4o",
        ModelPricing {
            input_per_mtok: dec("2.50"),
            output_per_mtok: dec("10.00"),
            cached_input_per_mtok: Some(dec("1.25")),
            cache_write_per_mtok: None,
        },
    );
    m.insert(
        "openai/gpt-4o-mini",
        ModelPricing {
            input_per_mtok: dec("0.15"),
            output_per_mtok: dec("0.60"),
            cached_input_per_mtok: Some(dec("0.075")),
            cache_write_per_mtok: None,
        },
    );
    m.insert(
        "openai/o1-mini",
        ModelPricing {
            input_per_mtok: dec("3.00"),
            output_per_mtok: dec("12.00"),
            cached_input_per_mtok: Some(dec("1.50")),
            cache_write_per_mtok: None,
        },
    );

    // Google via OpenRouter
    m.insert(
        "google/gemini-2.5-pro",
        ModelPricing {
            input_per_mtok: dec("1.25"),
            output_per_mtok: dec("5.00"),
            cached_input_per_mtok: None,
            cache_write_per_mtok: None,
        },
    );

    // DeepSeek via OpenRouter (cheap baseline)
    m.insert(
        "deepseek/deepseek-chat",
        ModelPricing {
            input_per_mtok: dec("0.27"),
            output_per_mtok: dec("1.10"),
            cached_input_per_mtok: Some(dec("0.07")),
            cache_write_per_mtok: None,
        },
    );

    m
}

#[cfg(test)]
mod tests {
    use super::pricing_for;
    use rust_decimal::Decimal;
    use std::str::FromStr;

    #[test]
    fn known_models_resolve() {
        let p = pricing_for("anthropic/claude-sonnet-4-6").unwrap();
        assert_eq!(p.input_per_mtok, Decimal::from_str("3.00").unwrap());
        assert_eq!(p.output_per_mtok, Decimal::from_str("15.00").unwrap());
    }

    #[test]
    fn unknown_model_returns_none() {
        assert!(pricing_for("unknown/foo").is_none());
    }
}

/// Per-model pricing in USD per million tokens.
pub struct ModelPricing {
    pub input_cost_per_million: f64,
    pub output_cost_per_million: f64,
}

/// Returns pricing for a known model, or `None` for unknown models.
///
/// Models in the table below use these rates. Anything else falls back to the
/// `llm-prices` table, which covers Claude models, newer OpenAI and Gemini
/// models, and provider-prefixed ids such as `anthropic/claude-sonnet-4-6`.
pub fn model_pricing(model: &str) -> Option<ModelPricing> {
    table_pricing(model).or_else(|| fallback_pricing(model))
}

fn fallback_pricing(model: &str) -> Option<ModelPricing> {
    let price = llm_prices::lookup(model)?;
    Some(ModelPricing {
        input_cost_per_million: price.input?,
        output_cost_per_million: price.output?,
    })
}

fn table_pricing(model: &str) -> Option<ModelPricing> {
    let (input, output) = match model {
        // GPT-4.1 family
        "gpt-4.1" => (2.0, 8.0),
        "gpt-4.1-mini" => (0.4, 1.6),
        "gpt-4.1-nano" => (0.1, 0.4),
        // GPT-4o family
        "gpt-4o" => (2.5, 10.0),
        "gpt-4o-mini" => (0.15, 0.6),
        // GPT-5 family
        "gpt-5" => (1.25, 10.0),
        "gpt-5-mini" => (0.25, 2.0),
        "gpt-5-nano" => (0.05, 0.4),
        "gpt-5.1" => (1.25, 10.0),
        "gpt-5.2" => (1.75, 14.0),
        "gpt-5.3" => (1.75, 14.0),
        "gpt-5.4" => (2.5, 15.0),
        "gpt-5.4-mini" => (0.75, 4.5),
        "gpt-5.4-nano" => (0.2, 1.25),
        // Gemini family
        "gemini-2.0-flash" => (0.1, 0.4),
        "gemini-2.5-flash" => (0.3, 2.5),
        "gemini-2.5-flash-lite" => (0.1, 0.4),
        "gemini-2.5-pro" => (1.25, 10.0),
        "gemini-3-flash-preview" => (0.5, 3.0),
        "gemini-3.1-flash-lite-preview" => (0.25, 1.5),
        "gemini-3.1-pro-preview" => (2.0, 12.0),
        // DeepSeek family (cache-miss / undiscounted rates)
        "deepseek-v4-flash" => (0.14, 0.28),
        "deepseek-v4-pro" => (1.74, 3.48),
        // Grok family
        "grok-4.3" => (1.25, 2.50),
        _ => return None,
    };
    Some(ModelPricing {
        input_cost_per_million: input,
        output_cost_per_million: output,
    })
}

/// Calculate cost in USD from token counts.
/// Returns 0.0 for unknown models.
pub fn calculate_cost(model: &str, input_tokens: u64, output_tokens: u64) -> f64 {
    let Some(pricing) = model_pricing(model) else {
        return 0.0;
    };
    let input_cost = (input_tokens as f64 / 1_000_000.0) * pricing.input_cost_per_million;
    let output_cost = (output_tokens as f64 / 1_000_000.0) * pricing.output_cost_per_million;
    input_cost + output_cost
}

/// Format a cost value as `$X.XXXX`.
pub fn format_cost(cost: f64) -> String {
    format!("${cost:.4}")
}

/// List of known models with pricing data.
/// This is NOT a whitelist — any model name is accepted. These are models
/// for which we can display cost estimates and show in the TUI model picker.
pub const KNOWN_MODELS: &[&str] = &[
    "gpt-4.1",
    "gpt-4.1-mini",
    "gpt-4.1-nano",
    "gpt-4o",
    "gpt-4o-mini",
    "gpt-5",
    "gpt-5-mini",
    "gpt-5-nano",
    "gpt-5.1",
    "gpt-5.2",
    "gpt-5.3",
    "gpt-5.4",
    "gpt-5.4-mini",
    "gpt-5.4-nano",
    "gemini-2.0-flash",
    "gemini-2.5-flash",
    "gemini-2.5-flash-lite",
    "gemini-2.5-pro",
    "gemini-3-flash-preview",
    "gemini-3.1-flash-lite-preview",
    "gemini-3.1-pro-preview",
    "deepseek-v4-flash",
    "deepseek-v4-pro",
    "grok-4.3",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calculate_cost_known_model() {
        let cost = calculate_cost("gpt-5-mini", 1000, 500);
        let expected = (1000.0 / 1_000_000.0) * 0.25 + (500.0 / 1_000_000.0) * 2.0;
        assert!((cost - expected).abs() < f64::EPSILON);
    }

    #[test]
    fn calculate_cost_unknown_model() {
        assert_eq!(calculate_cost("unknown-model", 1000, 1000), 0.0);
    }

    #[test]
    fn format_cost_zero() {
        assert_eq!(format_cost(0.0), "$0.0000");
    }

    #[test]
    fn format_cost_small() {
        assert_eq!(format_cost(0.001), "$0.0010");
    }

    #[test]
    fn table_rates_take_precedence_over_fallback() {
        // llm-prices maps this id to a different row; the local rate must win.
        let p = model_pricing("deepseek-v4-flash").unwrap();
        assert_eq!(p.input_cost_per_million, 0.14);
        assert_eq!(p.output_cost_per_million, 0.28);
    }

    #[test]
    fn calculate_cost_falls_back_for_models_not_in_table() {
        for model in [
            "claude-sonnet-4-6",
            "anthropic/claude-sonnet-4-6",
            "gpt-5.5",
        ] {
            assert!(
                calculate_cost(model, 1000, 500) > 0.0,
                "no fallback pricing for {model}"
            );
        }
        let cost = calculate_cost("claude-sonnet-4-6", 1000, 500);
        let expected = (1000.0 / 1_000_000.0) * 3.0 + (500.0 / 1_000_000.0) * 15.0;
        assert!((cost - expected).abs() < 1e-12);
    }

    #[test]
    fn all_supported_models_have_pricing() {
        for model in KNOWN_MODELS {
            assert!(
                model_pricing(model).is_some(),
                "missing pricing for {model}"
            );
        }
    }
}

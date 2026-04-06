/// Per-model pricing in USD per million tokens.
pub struct ModelPricing {
    pub input_cost_per_million: f64,
    pub output_cost_per_million: f64,
}

/// Returns pricing for a known model, or `None` for unknown models.
pub fn model_pricing(model: &str) -> Option<ModelPricing> {
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
        "gpt-5.4-pro" => (30.0, 180.0),
        "gpt-5.4-mini" => (0.75, 4.5),
        "gpt-5.4-nano" => (0.2, 1.25),
        // Gemini family
        "gemini-2.0-flash" => (0.1, 0.4),
        "gemini-2.5-flash" => (0.3, 2.5),
        "gemini-2.5-flash-lite" => (0.1, 0.4),
        "gemini-2.5-pro" => (1.25, 10.0),
        "gemini-3-flash-preview" => (0.5, 3.0),
        "gemini-3.1-pro-preview" => (2.0, 12.0),
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

/// Format a cost display string with token counts.
pub fn format_cost_display(total_cost: f64, input_tokens: u64, output_tokens: u64) -> String {
    format!(
        "  Cost: {} ({input_tokens} input + {output_tokens} output tokens)",
        format_cost(total_cost),
    )
}

/// List of all known supported model names.
pub const SUPPORTED_MODELS: &[&str] = &[
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
    "gpt-5.4-pro",
    "gpt-5.4-mini",
    "gpt-5.4-nano",
    "gemini-2.0-flash",
    "gemini-2.5-flash",
    "gemini-2.5-flash-lite",
    "gemini-2.5-pro",
    "gemini-3-flash-preview",
    "gemini-3.1-pro-preview",
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
    fn format_cost_display_string() {
        let s = format_cost_display(0.005, 1000, 500);
        assert!(s.contains("$0.0050"));
        assert!(s.contains("1000 input"));
        assert!(s.contains("500 output"));
    }

    #[test]
    fn all_supported_models_have_pricing() {
        for model in SUPPORTED_MODELS {
            assert!(
                model_pricing(model).is_some(),
                "missing pricing for {model}"
            );
        }
    }
}

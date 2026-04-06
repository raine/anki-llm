use anyhow::{Result, bail};

use crate::llm::pricing::SUPPORTED_MODELS;
use crate::llm::provider::{self, api_key_for_model, resolve_model};

/// Validated runtime configuration for LLM operations.
#[derive(Debug)]
pub struct RuntimeConfig {
    pub model: String,
    pub api_key: String,
    pub api_base_url: Option<String>,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u64>,
    pub batch_size: u32,
    pub retries: u32,
    pub dry_run: bool,
}

pub struct RuntimeConfigArgs<'a> {
    pub model: Option<&'a str>,
    pub batch_size: Option<u32>,
    pub max_tokens: Option<u64>,
    pub temperature: Option<f64>,
    pub retries: u32,
    pub dry_run: bool,
}

/// Build a RuntimeConfig from CLI args and environment.
/// Validates model name, resolves API key, applies temperature rules.
pub fn build_runtime_config(args: RuntimeConfigArgs<'_>) -> Result<RuntimeConfig> {
    let model = resolve_model(args.model);

    if !SUPPORTED_MODELS.contains(&model.as_str()) {
        bail!(
            "invalid model: {model}\nSupported models: {}",
            SUPPORTED_MODELS.join(", ")
        );
    }

    let api_key = if args.dry_run {
        "dry-run".to_string()
    } else {
        let provider = provider::provider_config(&model);
        match api_key_for_model(&model) {
            Some(key) => key,
            None => bail!(
                "API key required: set {} environment variable\n\
                 Tip: for model '{model}', set {}\n\
                 Or use --dry-run to preview without an API key",
                provider.api_key_env,
                provider.api_key_env,
            ),
        }
    };

    let provider = provider::provider_config(&model);

    let temperature = if provider::omit_temperature(&model) {
        None
    } else {
        args.temperature
    };

    Ok(RuntimeConfig {
        model,
        api_key,
        api_base_url: provider.base_url,
        temperature,
        max_tokens: args.max_tokens,
        batch_size: args.batch_size.unwrap_or(5),
        retries: args.retries,
        dry_run: args.dry_run,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_model() {
        let result = build_runtime_config(RuntimeConfigArgs {
            model: Some("unknown-model"),
            batch_size: None,
            max_tokens: None,
            temperature: None,
            retries: 2,
            dry_run: true,
        });
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid model"));
    }

    #[test]
    fn dry_run_skips_api_key() {
        let config = build_runtime_config(RuntimeConfigArgs {
            model: Some("gpt-5-mini"),
            batch_size: None,
            max_tokens: None,
            temperature: Some(0.7),
            retries: 2,
            dry_run: true,
        })
        .unwrap();
        assert_eq!(config.api_key, "dry-run");
        assert_eq!(config.model, "gpt-5-mini");
    }

    #[test]
    fn gpt5_omits_temperature() {
        let config = build_runtime_config(RuntimeConfigArgs {
            model: Some("gpt-5"),
            batch_size: None,
            max_tokens: None,
            temperature: Some(0.7),
            retries: 2,
            dry_run: true,
        })
        .unwrap();
        assert!(config.temperature.is_none());
    }

    #[test]
    fn non_gpt5_preserves_temperature() {
        let config = build_runtime_config(RuntimeConfigArgs {
            model: Some("gemini-2.5-flash"),
            batch_size: None,
            max_tokens: None,
            temperature: Some(0.7),
            retries: 2,
            dry_run: true,
        })
        .unwrap();
        assert_eq!(config.temperature, Some(0.7));
    }

    #[test]
    fn default_batch_size() {
        let config = build_runtime_config(RuntimeConfigArgs {
            model: Some("gpt-5-mini"),
            batch_size: None,
            max_tokens: None,
            temperature: None,
            retries: 2,
            dry_run: true,
        })
        .unwrap();
        assert_eq!(config.batch_size, 5);
    }
}

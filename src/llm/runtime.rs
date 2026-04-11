use anyhow::{Result, bail};

use crate::llm::provider::{self, api_key_for_model, resolve_model};

/// Validated runtime configuration for LLM operations.
#[derive(Debug)]
pub struct RuntimeConfig {
    pub model: String,
    pub api_key: Option<String>,
    pub api_base_url: Option<String>,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u64>,
    pub batch_size: u32,
    pub retries: u32,
    pub dry_run: bool,
}

pub struct RuntimeConfigArgs<'a> {
    pub model: Option<&'a str>,
    pub api_base_url: Option<&'a str>,
    pub api_key: Option<&'a str>,
    pub batch_size: Option<u32>,
    pub max_tokens: Option<u64>,
    pub temperature: Option<f64>,
    pub retries: u32,
    pub dry_run: bool,
}

/// Build a RuntimeConfig from CLI args and environment.
/// Validates temperature range. Resolves API key and base URL with precedence:
/// CLI flag > environment variable > config file > auto-detect.
pub fn build_runtime_config(args: RuntimeConfigArgs<'_>) -> Result<RuntimeConfig> {
    let model = resolve_model(args.model);

    if let Some(t) = args.temperature
        && !(0.0..=2.0).contains(&t)
    {
        bail!("temperature must be between 0 and 2, got {t}");
    }

    // Resolve base URL: CLI flag > ANKI_LLM_API_BASE_URL env > config file > provider auto-detect
    // Track whether the user explicitly configured a custom endpoint, because
    // custom endpoints don't require an API key (the server decides).
    let (api_base_url, custom_endpoint) = if let Some(url) = args.api_base_url {
        (Some(url.to_string()), true)
    } else if let Ok(url) = std::env::var("ANKI_LLM_API_BASE_URL") {
        (Some(url), true)
    } else if let Ok(config) = crate::config::store::read_config()
        && let Some(ref url) = config.api_base_url
    {
        (Some(url.clone()), true)
    } else {
        // Fall back to provider auto-detection (Gemini, OpenAI)
        (provider::provider_config(&model).base_url, false)
    };

    // Resolve API key: CLI flag > ANKI_LLM_API_KEY env > provider-specific env var.
    // When a custom endpoint is configured, never fall through to provider-specific
    // env vars (OPENAI_API_KEY, GEMINI_API_KEY) — that would leak credentials to
    // a third-party server.
    let api_key = if args.dry_run {
        None
    } else if let Some(key) = args.api_key {
        Some(key.to_string())
    } else if let Ok(key) = std::env::var("ANKI_LLM_API_KEY") {
        if key.trim().is_empty() {
            None
        } else {
            Some(key)
        }
    } else if custom_endpoint {
        None
    } else {
        api_key_for_model(&model)
    };

    // Only require an API key for auto-detected providers (OpenAI, Gemini).
    // Custom endpoints (user-configured base URL) are allowed without auth —
    // the server decides whether to reject unauthenticated requests.
    if !args.dry_run && api_key.is_none() && !custom_endpoint {
        let provider = provider::provider_config(&model);
        bail!(
            "API key required: set ANKI_LLM_API_KEY, {} environment variable, or pass --api-key\n\
             Tip: for custom endpoints, set --api-base-url and no key is needed\n\
             Or use --dry-run to preview without an API key",
            provider.api_key_env,
        );
    }

    let temperature = if provider::omit_temperature(&model) {
        None
    } else {
        args.temperature
    };

    Ok(RuntimeConfig {
        model,
        api_key,
        api_base_url,
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
    fn rejects_temperature_out_of_range() {
        for bad in [-0.1, 2.1, -1.0, 100.0] {
            let result = build_runtime_config(RuntimeConfigArgs {
                model: Some("gpt-5-mini"),
                api_base_url: None,
                api_key: None,
                batch_size: None,
                max_tokens: None,
                temperature: Some(bad),
                retries: 0,
                dry_run: true,
            });
            assert!(result.is_err(), "expected error for temperature={bad}");
            assert!(result.unwrap_err().to_string().contains("temperature"));
        }
    }

    #[test]
    fn accepts_temperature_boundary_values() {
        for ok in [0.0, 1.0, 2.0] {
            let result = build_runtime_config(RuntimeConfigArgs {
                model: Some("gpt-5-mini"),
                api_base_url: None,
                api_key: None,
                batch_size: None,
                max_tokens: None,
                temperature: Some(ok),
                retries: 0,
                dry_run: true,
            });
            assert!(result.is_ok(), "expected Ok for temperature={ok}");
        }
    }

    #[test]
    fn accepts_unknown_model_with_custom_endpoint() {
        // Custom endpoints don't require an API key.
        let result = build_runtime_config(RuntimeConfigArgs {
            model: Some("meta-llama/llama-3-8b-instruct"),
            api_base_url: Some("http://localhost:11434/v1"),
            api_key: None,
            batch_size: None,
            max_tokens: None,
            temperature: None,
            retries: 2,
            dry_run: false,
        });
        assert!(result.is_ok());
        let config = result.unwrap();
        assert_eq!(config.model, "meta-llama/llama-3-8b-instruct");
        assert_eq!(
            config.api_base_url.as_deref(),
            Some("http://localhost:11434/v1")
        );
    }

    #[test]
    fn dry_run_skips_api_key() {
        let config = build_runtime_config(RuntimeConfigArgs {
            model: Some("gpt-5-mini"),
            api_base_url: None,
            api_key: None,
            batch_size: None,
            max_tokens: None,
            temperature: Some(0.7),
            retries: 2,
            dry_run: true,
        })
        .unwrap();
        assert!(config.api_key.is_none());
        assert_eq!(config.model, "gpt-5-mini");
    }

    #[test]
    fn gpt5_omits_temperature() {
        let config = build_runtime_config(RuntimeConfigArgs {
            model: Some("gpt-5"),
            api_base_url: None,
            api_key: None,
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
            api_base_url: None,
            api_key: None,
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
            api_base_url: None,
            api_key: None,
            batch_size: None,
            max_tokens: None,
            temperature: None,
            retries: 2,
            dry_run: true,
        })
        .unwrap();
        assert_eq!(config.batch_size, 5);
    }

    #[test]
    fn custom_endpoint_does_not_require_api_key() {
        // Custom endpoints should never fail for missing API key, even when
        // no provider-specific env var is set (the key may still be picked up
        // from the environment, but that's fine — what matters is no error).
        let result = build_runtime_config(RuntimeConfigArgs {
            model: Some("llama3"),
            api_base_url: Some("http://my-server.lan:8080/v1"),
            api_key: None,
            batch_size: None,
            max_tokens: None,
            temperature: None,
            retries: 2,
            dry_run: false,
        });
        assert!(result.is_ok());
        let config = result.unwrap();
        assert_eq!(config.model, "llama3");
        assert_eq!(
            config.api_base_url.as_deref(),
            Some("http://my-server.lan:8080/v1")
        );
    }

    #[test]
    fn cli_api_key_takes_precedence() {
        let config = build_runtime_config(RuntimeConfigArgs {
            model: Some("custom-model"),
            api_base_url: Some("https://openrouter.ai/api/v1"),
            api_key: Some("sk-test-key"),
            batch_size: None,
            max_tokens: None,
            temperature: None,
            retries: 2,
            dry_run: false,
        })
        .unwrap();
        assert_eq!(config.api_key.as_deref(), Some("sk-test-key"));
        assert_eq!(
            config.api_base_url.as_deref(),
            Some("https://openrouter.ai/api/v1")
        );
    }
}

use std::env;

use crate::config::store::read_config;

/// Provider configuration resolved from a model name.
pub struct ProviderConfig {
    /// Base URL override (None = provider default).
    pub base_url: Option<String>,
    /// Environment variable name for the API key.
    pub api_key_env: &'static str,
}

/// Determine provider config from model name prefix.
pub fn provider_config(model: &str) -> ProviderConfig {
    if model.starts_with("gemini-") {
        ProviderConfig {
            base_url: Some("https://generativelanguage.googleapis.com/v1beta/openai".to_string()),
            api_key_env: "GEMINI_API_KEY",
        }
    } else {
        ProviderConfig {
            base_url: None,
            api_key_env: "OPENAI_API_KEY",
        }
    }
}

/// Get the API key for the given model from the environment.
/// Returns `None` if the env var is unset or empty/whitespace-only.
pub fn api_key_for_model(model: &str) -> Option<String> {
    let config = provider_config(model);
    env::var(config.api_key_env)
        .ok()
        .filter(|k| !k.trim().is_empty())
}

/// Returns models from `SUPPORTED_MODELS` for which an API key is available.
/// If `include_all` is true (e.g. dry-run mode), returns all models.
pub fn available_models(include_all: bool) -> Vec<&'static str> {
    use crate::llm::pricing::SUPPORTED_MODELS;
    if include_all {
        return SUPPORTED_MODELS.to_vec();
    }
    SUPPORTED_MODELS
        .iter()
        .copied()
        .filter(|model| api_key_for_model(model).is_some())
        .collect()
}

/// Resolve which model to use.
/// Priority: CLI flag → config file → env-var auto-detect.
pub fn resolve_model(user_model: Option<&str>) -> String {
    if let Some(m) = user_model {
        return m.to_string();
    }
    if let Ok(config) = read_config()
        && let Some(serde_json::Value::String(m)) = config.get("model")
        && !m.is_empty()
    {
        return m.clone();
    }
    if env::var("GEMINI_API_KEY").is_ok() {
        "gemini-2.5-flash".to_string()
    } else {
        "gpt-5".to_string()
    }
}

/// Returns true if the model should omit the temperature parameter.
/// GPT-5 models don't support it.
pub fn omit_temperature(model: &str) -> bool {
    model.starts_with("gpt-5")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpt_model_uses_openai() {
        let config = provider_config("gpt-5-mini");
        assert_eq!(config.api_key_env, "OPENAI_API_KEY");
        assert!(config.base_url.is_none());
    }

    #[test]
    fn gemini_model_uses_gemini() {
        let config = provider_config("gemini-2.5-flash");
        assert_eq!(config.api_key_env, "GEMINI_API_KEY");
        assert!(config.base_url.is_some());
        assert!(
            config
                .base_url
                .unwrap()
                .contains("generativelanguage.googleapis.com")
        );
    }

    #[test]
    fn unknown_prefix_defaults_to_openai() {
        let config = provider_config("custom-model");
        assert_eq!(config.api_key_env, "OPENAI_API_KEY");
    }

    #[test]
    fn omit_temperature_gpt5() {
        assert!(omit_temperature("gpt-5"));
        assert!(omit_temperature("gpt-5-mini"));
        assert!(omit_temperature("gpt-5.1"));
    }

    #[test]
    fn preserve_temperature_non_gpt5() {
        assert!(!omit_temperature("gpt-4.1"));
        assert!(!omit_temperature("gemini-2.5-flash"));
    }
}

use std::env;

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
pub fn api_key_for_model(model: &str) -> Option<String> {
    let config = provider_config(model);
    env::var(config.api_key_env).ok()
}

/// Resolve which model to use. If `user_model` is provided, use it.
/// Otherwise auto-detect: prefer Gemini if `GEMINI_API_KEY` is set,
/// else default to `gpt-5`.
pub fn resolve_model(user_model: Option<&str>) -> String {
    if let Some(m) = user_model {
        return m.to_string();
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

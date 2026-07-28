mod request;
mod response;
mod streaming;
mod transport;

pub use request::{JsonSchema, ResponseFormat};
pub use response::ChatCompletionResult;

use std::time::Instant;

use request::{structured_chat_request, thinking_stream_chat_request};
use response::{ChatResponse, effective_usage};
use streaming::read_stream_completion_with_idle_timeout;
use transport::TIMEOUT_SECS;

use crate::llm::error::LlmError;
use crate::llm::provider::{self, ThinkingFormat};
use crate::llm::runtime::RuntimeConfig;

const DEFAULT_OPENAI_BASE: &str = "https://api.openai.com/v1";
const STREAM_READ_TIMEOUT_SECS: u64 = 30;

pub struct LlmClient {
    base_url: String,
    api_key: Option<String>,
    reasoning_effort: Option<String>,
    agent: ureq::Agent,
    gemini_thinking_enabled: bool,
}

impl LlmClient {
    /// Create a new LLM client from runtime config.
    ///
    /// Uses a ureq Agent with a global timeout (ureq v3 requires agent-level
    /// timeout configuration, not per-request).
    pub fn from_config(config: &RuntimeConfig) -> Self {
        let base_url = config
            .api_base_url
            .clone()
            .unwrap_or_else(|| DEFAULT_OPENAI_BASE.to_string())
            .trim_end_matches('/')
            .to_string();

        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(TIMEOUT_SECS)))
            .build()
            .into();

        Self {
            base_url,
            api_key: config.api_key.clone(),
            reasoning_effort: config.reasoning_effort.clone(),
            agent,
            gemini_thinking_enabled: config.gemini_thinking_enabled,
        }
    }

    pub fn supports_thinking_stream(&self, model: &str) -> bool {
        self.thinking_format_for_request(model).is_some()
    }

    fn thinking_format_for_request(&self, model: &str) -> Option<ThinkingFormat> {
        match provider::thinking_format_for(model, &self.base_url) {
            Some(ThinkingFormat::GeminiThoughtTags) if self.gemini_thinking_enabled => {
                Some(ThinkingFormat::GeminiThoughtTags)
            }
            Some(ThinkingFormat::ReasoningContent) => Some(ThinkingFormat::ReasoningContent),
            _ => None,
        }
    }

    /// Send a chat completion request with a single user message.
    pub fn chat_completion(
        &self,
        model: &str,
        prompt: &str,
        temperature: Option<f64>,
        max_tokens: Option<u64>,
    ) -> Result<ChatCompletionResult, LlmError> {
        self.chat_completion_structured(model, None, prompt, temperature, max_tokens, None)
    }

    pub fn chat_completion_with_thinking(
        &self,
        model: &str,
        prompt: &str,
        temperature: Option<f64>,
        max_tokens: Option<u64>,
        mut on_reset: impl FnMut(),
        mut on_thinking: impl FnMut(&str),
    ) -> Result<ChatCompletionResult, LlmError> {
        let Some(format) = self.thinking_format_for_request(model) else {
            return self.chat_completion(model, prompt, temperature, max_tokens);
        };
        on_reset();

        let body = thinking_stream_chat_request(
            model,
            prompt,
            self.reasoning_effort.as_deref(),
            temperature,
            max_tokens,
            format,
        );
        let response = self.send_chat_request(body, true)?;
        let result = read_stream_completion_with_idle_timeout(
            response.into_parts().1.into_reader(),
            format,
            STREAM_READ_TIMEOUT_SECS,
            &mut on_thinking,
            Instant::now,
        )?;
        if result.content.is_empty() {
            return Err(LlmError::Decode("no content in response".into()));
        }
        Ok(result)
    }

    /// Send a chat completion request with optional system message and structured output.
    pub fn chat_completion_structured(
        &self,
        model: &str,
        system_prompt: Option<&str>,
        user_prompt: &str,
        temperature: Option<f64>,
        max_tokens: Option<u64>,
        response_format: Option<&ResponseFormat>,
    ) -> Result<ChatCompletionResult, LlmError> {
        let body = structured_chat_request(
            model,
            system_prompt,
            user_prompt,
            self.reasoning_effort.as_deref(),
            temperature,
            max_tokens,
            response_format,
        );

        let mut response = self.send_chat_request(body, false)?;
        let resp: ChatResponse = response
            .body_mut()
            .read_json()
            .map_err(|e| LlmError::Decode(e.to_string()))?;

        let content = resp
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .ok_or_else(|| LlmError::Decode("no content in response".into()))?;

        Ok(ChatCompletionResult {
            content,
            usage: resp.usage.map(effective_usage),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_client(base_url: &str, gemini_thinking_enabled: bool) -> LlmClient {
        LlmClient {
            base_url: base_url.to_string(),
            api_key: None,
            reasoning_effort: None,
            agent: ureq::Agent::new_with_defaults(),
            gemini_thinking_enabled,
        }
    }

    #[test]
    fn gemini_thinking_config_gates_gemini_only() {
        let gemini = test_client(
            "https://generativelanguage.googleapis.com/v1beta/openai",
            false,
        );
        assert!(!gemini.supports_thinking_stream("gemini-2.5-flash"));

        let deepseek = test_client("https://api.deepseek.com", false);
        assert!(deepseek.supports_thinking_stream("deepseek-v4-pro"));
    }

    #[test]
    fn gemini_thinking_enabled_allows_gemini_streaming() {
        let gemini = test_client(
            "https://generativelanguage.googleapis.com/v1beta/openai",
            true,
        );
        assert!(gemini.supports_thinking_stream("gemini-2.5-flash"));
    }
}

use serde::{Deserialize, Serialize};

use super::error::LlmError;
use crate::llm::provider;
use crate::llm::runtime::RuntimeConfig;

const DEFAULT_OPENAI_BASE: &str = "https://api.openai.com/v1";
const TIMEOUT_SECS: u64 = 90;

#[derive(Debug, Clone, Serialize)]
pub struct JsonSchema {
    pub name: String,
    pub schema: serde_json::Value,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub strict: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseFormat {
    JsonSchema { json_schema: JsonSchema },
}

#[derive(Debug, Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<&'a ResponseFormat>,
}

#[derive(Debug, Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
    usage: Option<ChatUsage>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatChoiceMessage,
}

#[derive(Debug, Deserialize)]
struct ChatChoiceMessage {
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ChatUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

/// Result of a chat completion call.
pub struct ChatCompletionResult {
    pub content: String,
    pub usage: Option<ChatUsage>,
}

pub struct LlmClient {
    base_url: String,
    api_key: String,
    agent: ureq::Agent,
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
            .unwrap_or_else(|| DEFAULT_OPENAI_BASE.to_string());

        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(TIMEOUT_SECS)))
            .build()
            .into();

        Self {
            base_url,
            api_key: config.api_key.clone(),
            agent,
        }
    }

    /// Create a client for a specific model, resolving the correct provider
    /// base URL and API key. Returns `None` if no API key is available.
    pub fn for_model(model: &str) -> Option<Self> {
        let config = provider::provider_config(model);
        let api_key = provider::api_key_for_model(model)?;
        let base_url = config
            .base_url
            .unwrap_or_else(|| DEFAULT_OPENAI_BASE.to_string());

        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(TIMEOUT_SECS)))
            .build()
            .into();

        Some(Self {
            base_url,
            api_key,
            agent,
        })
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
        let url = format!("{}/chat/completions", self.base_url);

        let mut messages = Vec::new();
        if let Some(sys) = system_prompt {
            messages.push(ChatMessage {
                role: "system",
                content: sys,
            });
        }
        messages.push(ChatMessage {
            role: "user",
            content: user_prompt,
        });

        let body = ChatRequest {
            model,
            messages,
            temperature,
            max_tokens,
            response_format,
        };

        let mut response = self
            .agent
            .post(&url)
            .header("Authorization", &format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .send_json(&body)
            .map_err(|e| match e {
                // 429 and 5xx are transient; other 4xx are permanent (bad key,
                // invalid model, malformed request) and should not be retried.
                ureq::Error::StatusCode(429) => {
                    LlmError::Http("HTTP 429: rate limited".to_string())
                }
                ureq::Error::StatusCode(code) if code >= 500 => {
                    LlmError::Http(format!("HTTP {code}: server error"))
                }
                ureq::Error::StatusCode(code) => {
                    LlmError::Api(format!("HTTP {code}: non-retryable error"))
                }
                other => LlmError::Http(other.to_string()),
            })?;

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
            usage: resp.usage,
        })
    }
}

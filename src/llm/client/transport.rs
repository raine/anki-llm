use super::LlmClient;
use super::request::ChatRequest;
use crate::llm::error::LlmError;

pub(super) const TIMEOUT_SECS: u64 = 90;

impl LlmClient {
    pub(super) fn send_chat_request(
        &self,
        body: ChatRequest<'_>,
        streaming: bool,
    ) -> Result<ureq::http::Response<ureq::Body>, LlmError> {
        let url = format!("{}/chat/completions", self.base_url);
        let mut request = self
            .agent
            .post(&url)
            .header("Content-Type", "application/json");

        if streaming {
            request = request
                .config()
                .timeout_global(None)
                .http_status_as_error(false)
                .build();
        }

        // Only send Authorization header when we have an API key.
        // Local servers (Ollama, llama.cpp) often reject unexpected auth headers.
        if let Some(ref key) = self.api_key {
            request = request.header("Authorization", &format!("Bearer {key}"));
        }

        let body = serde_json::to_vec(&body).map_err(|e| LlmError::Decode(e.to_string()))?;
        let mut response = request.send(&body[..]).map_err(|e| match e {
            // 429 and 5xx are transient; other 4xx are permanent (bad key,
            // invalid model, malformed request) and should not be retried.
            ureq::Error::StatusCode(429) => LlmError::Http("HTTP 429: rate limited".to_string()),
            ureq::Error::StatusCode(code) if code >= 500 => {
                LlmError::Http(format!("HTTP {code}: server error"))
            }
            ureq::Error::StatusCode(code) => {
                LlmError::Api(format!("HTTP {code}: non-retryable error"))
            }
            other => LlmError::Http(other.to_string()),
        })?;

        if streaming && !response.status().is_success() {
            let status = response.status();
            let body = response.body_mut().read_to_string().unwrap_or_default();
            if status.as_u16() == 429 || status.is_server_error() {
                return Err(LlmError::Http(format!("HTTP {status}: {body}")));
            }
            return Err(LlmError::Api(format!("HTTP {status}: {body}")));
        }

        Ok(response)
    }
}

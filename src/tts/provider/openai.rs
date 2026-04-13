use std::io::Read;
use std::time::Duration;

use serde::Serialize;

use super::{SynthesisRequest, TextFormat, TtsProvider};
use crate::tts::error::TtsError;

const DEFAULT_BASE: &str = "https://api.openai.com/v1";
const TIMEOUT_SECS: u64 = 120;
const DEFAULT_MODEL: &str = "gpt-4o-mini-tts";

pub struct OpenAiTtsProvider {
    base_url: String,
    api_key: Option<String>,
    agent: ureq::Agent,
}

impl OpenAiTtsProvider {
    pub fn new(api_key: Option<String>, base_url: Option<String>) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(TIMEOUT_SECS)))
            .build()
            .into();
        Self {
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE.to_string()),
            api_key,
            agent,
        }
    }
}

#[derive(Serialize)]
struct SpeechRequest<'a> {
    model: &'a str,
    input: &'a str,
    voice: &'a str,
    response_format: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    speed: Option<f32>,
}

impl TtsProvider for OpenAiTtsProvider {
    fn id(&self) -> &'static str {
        "openai"
    }

    fn text_format(&self) -> TextFormat {
        TextFormat::PlainText
    }

    fn synthesize(&self, req: &SynthesisRequest) -> Result<Vec<u8>, TtsError> {
        let url = format!("{}/audio/speech", self.base_url);
        let body = SpeechRequest {
            model: req.model.as_deref().unwrap_or(DEFAULT_MODEL),
            input: &req.payload,
            voice: &req.voice,
            response_format: req.format.ext(),
            speed: req.speed,
        };

        let mut request = self
            .agent
            .post(&url)
            .header("Content-Type", "application/json");
        if let Some(ref k) = self.api_key {
            request = request.header("Authorization", &format!("Bearer {k}"));
        }

        let mut response = request.send_json(&body).map_err(|e| match e {
            ureq::Error::StatusCode(429) => {
                TtsError::Transient("HTTP 429: rate limited".to_string())
            }
            ureq::Error::StatusCode(code) if code >= 500 => {
                TtsError::Transient(format!("HTTP {code}: server error"))
            }
            ureq::Error::StatusCode(code) => {
                TtsError::Permanent(format!("HTTP {code}: non-retryable error"))
            }
            other => TtsError::Transient(other.to_string()),
        })?;

        let mut buf = Vec::new();
        response
            .body_mut()
            .as_reader()
            .read_to_end(&mut buf)
            .map_err(|e| TtsError::Transient(format!("body read failed: {e}")))?;
        if buf.is_empty() {
            // Treat a zero-byte 200 OK as a transport glitch rather than a
            // permanent client error — the server accepted the request but
            // returned nothing, which is almost always retryable.
            return Err(TtsError::Transient("empty audio response".to_string()));
        }
        Ok(buf)
    }
}

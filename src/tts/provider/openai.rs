use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;

use crate::llm::logger::LlmLogger;

use super::{RenderProfile, SynthesisRequest, TtsProvider};
use crate::tts::error::TtsError;

const DEFAULT_BASE: &str = "https://api.openai.com/v1";
const TIMEOUT_SECS: u64 = 120;
const DEFAULT_MODEL: &str = "gpt-4o-mini-tts";

pub struct OpenAiTtsProvider {
    base_url: String,
    api_key: Option<String>,
    agent: ureq::Agent,
    logger: Option<Arc<LlmLogger>>,
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
            logger: None,
        }
    }

    pub fn with_logger(mut self, logger: Option<Arc<LlmLogger>>) -> Self {
        self.logger = logger;
        self
    }

    fn request_body<'a>(&self, req: &'a SynthesisRequest) -> SpeechRequest<'a> {
        SpeechRequest {
            model: req.model.as_deref().unwrap_or(DEFAULT_MODEL),
            input: &req.payload,
            voice: &req.voice,
            response_format: req.format.ext(),
            speed: req.speed,
            instructions: req.instructions.as_deref(),
        }
    }

    fn log_request(&self, body: &SpeechRequest<'_>, cached: bool) {
        if let Some(logger) = &self.logger
            && logger.is_active()
            && let Ok(json) = serde_json::to_string_pretty(body)
        {
            logger.log_tts_request(&json, cached);
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
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<&'a str>,
}

impl TtsProvider for OpenAiTtsProvider {
    fn id(&self) -> &'static str {
        "openai"
    }

    fn render_profile(&self) -> RenderProfile {
        RenderProfile::PlainText
    }

    fn log_cache_hit(&self, req: &SynthesisRequest) {
        self.log_request(&self.request_body(req), true);
    }

    fn synthesize(&self, req: &SynthesisRequest) -> Result<Vec<u8>, TtsError> {
        let url = format!("{}/audio/speech", self.base_url);
        let body = self.request_body(req);
        self.log_request(&body, false);

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tts::provider::{AudioFormat, TextFormat};
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    #[test]
    fn speech_request_sends_optional_instructions() {
        for instructions in [None, Some("Mluv česky.\nKeep {word} literal."), Some("")] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(&mut stream);
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                assert_eq!(line, "POST /audio/speech HTTP/1.1\r\n");
                let mut length = None;
                loop {
                    line.clear();
                    assert!(reader.read_line(&mut line).unwrap() > 0);
                    if line == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':')
                        && name.eq_ignore_ascii_case("content-length")
                    {
                        length = Some(value.trim().parse::<usize>().unwrap());
                    }
                }
                let mut body = vec![0; length.unwrap()];
                reader.read_exact(&mut body).unwrap();
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\naudio",
                    )
                    .unwrap();
                serde_json::from_slice::<serde_json::Value>(&body).unwrap()
            });
            let logs = tempfile::tempdir().unwrap();
            let log_path = logs.path().join("requests.log");
            let logger = Arc::new(LlmLogger::file_only(&log_path));
            let provider = OpenAiTtsProvider::new(Some("secret-test-key".into()), Some(base))
                .with_logger(Some(logger));
            let req = SynthesisRequest {
                payload: "v".into(),
                provider_id: "openai".into(),
                text_format: TextFormat::PlainText,
                voice: "alloy".into(),
                format: AudioFormat::Mp3,
                model: None,
                speed: None,
                instructions: instructions.map(String::from),
                endpoint: None,
            };
            assert_eq!(provider.synthesize(&req).unwrap(), b"audio");
            let body = server.join().unwrap();
            let logged = std::fs::read_to_string(&log_path).unwrap();
            let json = logged
                .split("--- TTS REQUEST (POST /audio/speech) ---\n")
                .nth(1)
                .unwrap()
                .trim();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(json).unwrap(),
                body
            );
            assert!(!logged.contains("secret-test-key"));
            assert!(!logged.contains("Authorization"));
            assert!(!logged.contains("CACHE HIT"));

            provider.log_cache_hit(&req);
            let logged = std::fs::read_to_string(&log_path).unwrap();
            assert_eq!(logged.matches("--- TTS REQUEST").count(), 1);
            let cached = logged
                .split("--- TTS CACHE HIT (no HTTP request) ---\n")
                .nth(1)
                .unwrap()
                .trim();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(cached).unwrap(),
                body
            );
            assert_eq!(body["input"], "v");
            assert_eq!(body["model"], DEFAULT_MODEL);
            match instructions {
                Some(text) => assert_eq!(body["instructions"], text),
                None => assert!(body.get("instructions").is_none()),
            }
        }
    }
}

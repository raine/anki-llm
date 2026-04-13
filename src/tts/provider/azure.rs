use std::io::Read;
use std::time::Duration;

use super::{SynthesisRequest, TextFormat, TtsProvider};
use crate::tts::error::TtsError;

const TIMEOUT_SECS: u64 = 120;

/// Azure Neural TTS REST endpoint format string. Per
/// <https://learn.microsoft.com/en-us/azure/ai-services/speech-service/rest-text-to-speech>,
/// the region is baked into the hostname.
fn endpoint_for(region: &str) -> String {
    format!("https://{region}.tts.speech.microsoft.com/cognitiveservices/v1")
}

/// Convenience: the region-only host prefix, used as the cache's endpoint
/// identity. This is what non-provider code threads into the cache key so
/// it doesn't need to know about `/cognitiveservices/v1`.
pub fn endpoint_identity(region: &str) -> String {
    format!("https://{region}.tts.speech.microsoft.com")
}

pub struct AzureTtsProvider {
    endpoint: String,
    subscription_key: String,
    agent: ureq::Agent,
}

impl AzureTtsProvider {
    pub fn new(subscription_key: String, region: String) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(TIMEOUT_SECS)))
            .build()
            .into();
        Self {
            endpoint: endpoint_for(&region),
            subscription_key,
            agent,
        }
    }
}

impl TtsProvider for AzureTtsProvider {
    fn id(&self) -> &'static str {
        "azure"
    }

    fn text_format(&self) -> TextFormat {
        TextFormat::Ssml
    }

    fn synthesize(&self, req: &SynthesisRequest) -> Result<Vec<u8>, TtsError> {
        // Azure rejects anything other than audio-24khz-48kbitrate-mono-mp3
        // for free-tier mp3. We lock to that format in headers — the cache
        // key already pins this via req.format.
        let output_format = match req.format.ext() {
            "mp3" => "audio-24khz-48kbitrate-mono-mp3",
            other => {
                return Err(TtsError::Permanent(format!(
                    "azure provider does not support format '{other}'"
                )));
            }
        };

        let mut response = self
            .agent
            .post(&self.endpoint)
            .header("Ocp-Apim-Subscription-Key", &self.subscription_key)
            .header("Content-Type", "application/ssml+xml")
            .header("X-Microsoft-OutputFormat", output_format)
            .header("User-Agent", "anki-llm")
            .send(req.payload.as_bytes())
            .map_err(map_ureq_error)?;

        let mut buf = Vec::new();
        response
            .body_mut()
            .as_reader()
            .read_to_end(&mut buf)
            .map_err(|e| TtsError::Transient(format!("body read failed: {e}")))?;
        if buf.is_empty() {
            return Err(TtsError::Transient(
                "empty audio response from azure".to_string(),
            ));
        }
        Ok(buf)
    }
}

/// Map ureq errors into TtsError. Ureq 3 returns HTTP error codes as
/// `Error::StatusCode(u16)` by default. Azure's documented status codes
/// for `/cognitiveservices/v1`:
///
/// - 400 Bad Request: malformed SSML / missing headers → permanent.
/// - 401 Unauthorized: bad subscription key → permanent.
/// - 403 Forbidden: key valid but no quota / wrong region → permanent.
/// - 415 Unsupported Media Type: wrong Content-Type → permanent.
/// - 429 Too Many Requests: rate limited → transient.
/// - 502/503/504 and other 5xx: transient.
/// - Any other network / timeout: transient.
fn map_ureq_error(e: ureq::Error) -> TtsError {
    match e {
        ureq::Error::StatusCode(429) => TtsError::Transient("HTTP 429: rate limited".to_string()),
        ureq::Error::StatusCode(code) if code >= 500 => {
            TtsError::Transient(format!("HTTP {code}: server error"))
        }
        ureq::Error::StatusCode(code) => {
            TtsError::Permanent(format!("HTTP {code}: non-retryable error"))
        }
        other => TtsError::Transient(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_for_region() {
        assert_eq!(
            endpoint_for("eastus"),
            "https://eastus.tts.speech.microsoft.com/cognitiveservices/v1"
        );
    }

    #[test]
    fn endpoint_identity_omits_path() {
        assert_eq!(
            endpoint_identity("westeurope"),
            "https://westeurope.tts.speech.microsoft.com"
        );
    }

    #[test]
    fn id_and_text_format() {
        let p = AzureTtsProvider::new("k".into(), "eastus".into());
        assert_eq!(p.id(), "azure");
        assert_eq!(p.text_format(), TextFormat::Ssml);
    }

    #[test]
    fn error_mapping_status_codes() {
        match map_ureq_error(ureq::Error::StatusCode(429)) {
            TtsError::Transient(_) => {}
            other => panic!("429 should be transient, got {other:?}"),
        }
        match map_ureq_error(ureq::Error::StatusCode(500)) {
            TtsError::Transient(_) => {}
            other => panic!("500 should be transient, got {other:?}"),
        }
        match map_ureq_error(ureq::Error::StatusCode(503)) {
            TtsError::Transient(_) => {}
            other => panic!("503 should be transient, got {other:?}"),
        }
        match map_ureq_error(ureq::Error::StatusCode(401)) {
            TtsError::Permanent(_) => {}
            other => panic!("401 should be permanent, got {other:?}"),
        }
        match map_ureq_error(ureq::Error::StatusCode(403)) {
            TtsError::Permanent(_) => {}
            other => panic!("403 should be permanent, got {other:?}"),
        }
        match map_ureq_error(ureq::Error::StatusCode(400)) {
            TtsError::Permanent(_) => {}
            other => panic!("400 should be permanent, got {other:?}"),
        }
    }
}

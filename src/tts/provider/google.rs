use std::io::Read;
use std::time::Duration;

use base64::Engine;
use serde::{Deserialize, Serialize};

use super::{AudioFormat, SynthesisRequest, TextFormat, TtsProvider};
use crate::tts::error::TtsError;

const ENDPOINT: &str = "https://texttospeech.googleapis.com/v1/text:synthesize";
const TIMEOUT_SECS: u64 = 120;

/// Stable host used as the cache endpoint identity. Google's TTS API is
/// a single global endpoint, so this never varies by region.
pub fn endpoint_identity() -> String {
    "https://texttospeech.googleapis.com".to_string()
}

pub struct GoogleTtsProvider {
    api_key: String,
    agent: ureq::Agent,
}

impl GoogleTtsProvider {
    pub fn new(api_key: String) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(TIMEOUT_SECS)))
            .build()
            .into();
        Self { api_key, agent }
    }
}

#[derive(Serialize)]
struct SpeechRequest<'a> {
    input: Input<'a>,
    voice: Voice<'a>,
    #[serde(rename = "audioConfig")]
    audio_config: AudioConfig<'a>,
}

#[derive(Serialize)]
struct Input<'a> {
    text: &'a str,
}

#[derive(Serialize)]
struct Voice<'a> {
    #[serde(rename = "languageCode")]
    language_code: String,
    name: &'a str,
}

#[derive(Serialize)]
struct AudioConfig<'a> {
    #[serde(rename = "audioEncoding")]
    audio_encoding: &'a str,
    #[serde(rename = "speakingRate", skip_serializing_if = "Option::is_none")]
    speaking_rate: Option<f32>,
}

#[derive(Deserialize)]
struct SpeechResponse {
    #[serde(rename = "audioContent")]
    audio_content: String,
}

/// Derive Google's `languageCode` from a full voice name. Google voice
/// names are always `<lang>-<REGION>-<style>...`, e.g. `ja-JP-Neural2-B`,
/// so `languageCode` is the first two hyphen-separated segments.
pub fn language_code_from_voice(voice: &str) -> Result<String, TtsError> {
    let mut parts = voice.splitn(3, '-');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(lang), Some(region), Some(_)) if !lang.is_empty() && !region.is_empty() => {
            Ok(format!("{lang}-{region}"))
        }
        _ => Err(TtsError::Permanent(format!(
            "google voice '{voice}' must be in the form '<lang>-<REGION>-<name>' \
             (e.g. 'ja-JP-Neural2-B')"
        ))),
    }
}

fn audio_encoding(fmt: AudioFormat) -> Result<&'static str, TtsError> {
    match fmt.ext() {
        "mp3" => Ok("MP3"),
        other => Err(TtsError::Permanent(format!(
            "google provider does not support format '{other}'"
        ))),
    }
}

impl TtsProvider for GoogleTtsProvider {
    fn id(&self) -> &'static str {
        "google"
    }

    fn text_format(&self) -> TextFormat {
        TextFormat::PlainText
    }

    fn synthesize(&self, req: &SynthesisRequest) -> Result<Vec<u8>, TtsError> {
        let language_code = language_code_from_voice(&req.voice)?;
        let encoding = audio_encoding(req.format)?;

        let body = SpeechRequest {
            input: Input { text: &req.payload },
            voice: Voice {
                language_code,
                name: &req.voice,
            },
            audio_config: AudioConfig {
                audio_encoding: encoding,
                speaking_rate: req.speed,
            },
        };

        let url = format!("{ENDPOINT}?key={}", self.api_key);
        let mut response = self
            .agent
            .post(&url)
            .header("Content-Type", "application/json")
            .header("User-Agent", "anki-llm")
            .send_json(&body)
            .map_err(map_ureq_error)?;

        let mut text = String::new();
        response
            .body_mut()
            .as_reader()
            .read_to_string(&mut text)
            .map_err(|e| TtsError::Transient(format!("body read failed: {e}")))?;
        let parsed: SpeechResponse = serde_json::from_str(&text)
            .map_err(|e| TtsError::Permanent(format!("invalid google response: {e}")))?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(parsed.audio_content.as_bytes())
            .map_err(|e| TtsError::Permanent(format!("google base64 decode failed: {e}")))?;
        if bytes.is_empty() {
            return Err(TtsError::Transient(
                "empty audio response from google".to_string(),
            ));
        }
        Ok(bytes)
    }
}

/// Map ureq errors into TtsError using the same split as Azure:
///
/// - 400/401/403/404: permanent (bad request, bad key, disabled API,
///   voice not found).
/// - 429: transient (rate limited).
/// - 5xx: transient (server error).
/// - network / timeout: transient.
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
    fn derives_language_code_from_voice() {
        assert_eq!(
            language_code_from_voice("ja-JP-Neural2-B").unwrap(),
            "ja-JP"
        );
        assert_eq!(
            language_code_from_voice("en-US-Wavenet-A").unwrap(),
            "en-US"
        );
        assert_eq!(
            language_code_from_voice("cmn-CN-Wavenet-A").unwrap(),
            "cmn-CN"
        );
    }

    #[test]
    fn rejects_voice_without_enough_segments() {
        assert!(language_code_from_voice("alloy").is_err());
        assert!(language_code_from_voice("ja-JP").is_err());
    }

    #[test]
    fn id_and_text_format() {
        let p = GoogleTtsProvider::new("fake".into());
        assert_eq!(p.id(), "google");
        assert_eq!(p.text_format(), TextFormat::PlainText);
    }

    #[test]
    fn error_mapping_status_codes() {
        assert!(matches!(
            map_ureq_error(ureq::Error::StatusCode(429)),
            TtsError::Transient(_)
        ));
        assert!(matches!(
            map_ureq_error(ureq::Error::StatusCode(500)),
            TtsError::Transient(_)
        ));
        assert!(matches!(
            map_ureq_error(ureq::Error::StatusCode(403)),
            TtsError::Permanent(_)
        ));
        assert!(matches!(
            map_ureq_error(ureq::Error::StatusCode(400)),
            TtsError::Permanent(_)
        ));
    }

    #[test]
    fn endpoint_identity_is_global() {
        assert_eq!(endpoint_identity(), "https://texttospeech.googleapis.com");
    }
}

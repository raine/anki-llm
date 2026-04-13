pub mod openai;

use std::sync::Arc;

use super::error::TtsError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioFormat {
    Mp3,
}

impl AudioFormat {
    pub fn ext(&self) -> &'static str {
        match self {
            AudioFormat::Mp3 => "mp3",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s.to_ascii_lowercase().as_str() {
            "mp3" => Ok(Self::Mp3),
            other => Err(format!("unsupported TTS format '{other}' (expected: mp3)")),
        }
    }
}

/// A single synthesis job, used both as the input to a TTS provider and as
/// the canonical cache key.
#[derive(Debug, Clone)]
pub struct SynthesisRequest {
    /// Normalized spoken text (see `tts::text::normalize`).
    pub text: String,
    /// Provider identifier (e.g. "openai"). Part of the cache key so
    /// switching providers doesn't return stale audio.
    pub provider_id: String,
    pub voice: String,
    pub format: AudioFormat,
    /// Provider-specific backing model (e.g. "gpt-4o-mini-tts"). Part of the
    /// cache key so upgrading the model invalidates old audio.
    pub model: Option<String>,
    pub speed: Option<f32>,
    /// Optional API base URL. Different backends (e.g. OpenAI vs. a
    /// self-hosted OpenAI-compatible server) can produce different audio
    /// for the same `(voice, model, text)` inputs, so the base URL is part
    /// of the cache key. `None` means "provider default".
    pub api_base_url: Option<String>,
}

pub trait TtsProvider: Send + Sync {
    fn id(&self) -> &'static str;
    fn synthesize(&self, req: &SynthesisRequest) -> Result<Vec<u8>, TtsError>;
}

/// Build a provider instance from its string identifier.
pub fn build(
    provider: &str,
    api_key: Option<String>,
    api_base_url: Option<String>,
) -> Result<Arc<dyn TtsProvider>, String> {
    match provider {
        "openai" => Ok(Arc::new(openai::OpenAiTtsProvider::new(
            api_key,
            api_base_url,
        ))),
        other => Err(format!("unknown TTS provider '{other}' (expected: openai)")),
    }
}

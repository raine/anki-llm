pub mod azure;
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

/// Payload format the provider expects. Part of the cache key so two
/// providers that happen to share a voice name can't collide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextFormat {
    PlainText,
    Ssml,
}

impl TextFormat {
    pub fn tag(&self) -> &'static str {
        match self {
            TextFormat::PlainText => "plain",
            TextFormat::Ssml => "ssml",
        }
    }
}

/// A prepared synthesis job: the payload has already been rendered from
/// the semantic IR into whatever string format the provider wants. The
/// provider POSTs `payload` verbatim; the cache hashes the exact bytes
/// along with the other identity fields below.
#[derive(Debug, Clone)]
pub struct SynthesisRequest {
    /// The exact string the provider will send. Already rendered (plain
    /// text for OpenAI, full SSML document for Azure) by the render layer
    /// before it ever reaches the provider.
    pub payload: String,
    /// Provider identifier (e.g. "openai", "azure"). Part of the cache
    /// key so switching providers doesn't return stale audio.
    pub provider_id: String,
    /// Format of `payload`. Part of the cache key so two providers that
    /// happen to share a voice name can't collide.
    pub text_format: TextFormat,
    pub voice: String,
    pub format: AudioFormat,
    /// Provider-specific backing model (e.g. "gpt-4o-mini-tts"). Part of
    /// the cache key so upgrading the model invalidates old audio. Not
    /// every provider uses this (Azure doesn't).
    pub model: Option<String>,
    pub speed: Option<f32>,
    /// Optional endpoint identity: for OpenAI-compatible providers this is
    /// `api_base_url`, for Azure it's `https://<region>.tts.speech.microsoft.com`.
    /// Part of the cache key so audio generated against a different
    /// endpoint doesn't get served under a matching voice/model key.
    pub endpoint: Option<String>,
}

pub trait TtsProvider: Send + Sync {
    fn id(&self) -> &'static str;
    fn text_format(&self) -> TextFormat;
    fn synthesize(&self, req: &SynthesisRequest) -> Result<Vec<u8>, TtsError>;
}

/// Typed provider identity used for constructing `Arc<dyn TtsProvider>`
/// instances. Providers that need different credentials, endpoints, or
/// regional routing own their own variant so `build` can't drop any
/// required field.
#[derive(Debug, Clone)]
pub enum ProviderSelection {
    OpenAi {
        api_key: Option<String>,
        api_base_url: Option<String>,
    },
    Azure {
        subscription_key: String,
        region: String,
    },
}

pub fn build(selection: ProviderSelection) -> Arc<dyn TtsProvider> {
    match selection {
        ProviderSelection::OpenAi {
            api_key,
            api_base_url,
        } => Arc::new(openai::OpenAiTtsProvider::new(api_key, api_base_url)),
        ProviderSelection::Azure {
            subscription_key,
            region,
        } => Arc::new(azure::AzureTtsProvider::new(subscription_key, region)),
    }
}

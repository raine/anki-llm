use anyhow::{Context, Result, bail};

use crate::config::store::read_config;

use super::provider::AudioFormat;
use super::spec::ResolvedProvider;

/// Fully-resolved TTS runtime config (CLI flags, env vars, and config file
/// merged). Mirrors the role of `llm::runtime::RuntimeConfig`.
pub struct TtsRuntime {
    pub provider: ResolvedProvider,
    pub voice: String,
    pub model: Option<String>,
    pub format: AudioFormat,
    pub speed: Option<f32>,
    pub batch_size: u32,
    pub retries: u32,
    pub force: bool,
    pub dry_run: bool,
}

pub struct TtsRuntimeArgs<'a> {
    pub provider: Option<&'a str>,
    pub voice: Option<&'a str>,
    pub tts_model: Option<&'a str>,
    pub format: Option<&'a str>,
    pub speed: Option<f32>,
    pub api_key: Option<&'a str>,
    pub api_base_url: Option<&'a str>,
    pub azure_region: Option<&'a str>,
    pub batch_size: u32,
    pub retries: u32,
    pub force: bool,
    pub dry_run: bool,
}

pub fn build_tts_runtime(args: TtsRuntimeArgs) -> Result<TtsRuntime> {
    let config = read_config().ok();

    let provider_name = args
        .provider
        .map(String::from)
        .or_else(|| config.as_ref().and_then(|c| c.tts_provider.clone()))
        .unwrap_or_else(|| "openai".to_string());

    let voice = args
        .voice
        .map(String::from)
        .or_else(|| config.as_ref().and_then(|c| c.tts_voice.clone()))
        .context("TTS voice is required (pass --voice or set `tts_voice` in the config)")?;

    let tts_model = args
        .tts_model
        .map(String::from)
        .or_else(|| config.as_ref().and_then(|c| c.tts_model.clone()));

    let format_str = args
        .format
        .map(String::from)
        .or_else(|| config.as_ref().and_then(|c| c.tts_format.clone()))
        .unwrap_or_else(|| "mp3".to_string());
    let format = AudioFormat::parse(&format_str).map_err(anyhow::Error::msg)?;

    let provider = match provider_name.as_str() {
        "openai" => {
            let api_base_url = args
                .api_base_url
                .map(String::from)
                .or_else(|| config.as_ref().and_then(|c| c.api_base_url.clone()));

            let api_key = args.api_key.map(String::from).or_else(|| {
                std::env::var("ANKI_LLM_API_KEY")
                    .ok()
                    .filter(|k| !k.trim().is_empty())
                    .or_else(|| {
                        std::env::var("OPENAI_API_KEY")
                            .ok()
                            .filter(|k| !k.trim().is_empty())
                    })
            });

            if api_key.is_none() && api_base_url.is_none() {
                bail!(
                    "OpenAI TTS requires an API key (set OPENAI_API_KEY or ANKI_LLM_API_KEY, or pass --api-key)"
                );
            }

            ResolvedProvider::OpenAi {
                api_key,
                api_base_url,
            }
        }
        "azure" => {
            let subscription_key = args
                .api_key
                .map(String::from)
                .or_else(|| {
                    std::env::var("AZURE_TTS_KEY")
                        .ok()
                        .filter(|k| !k.trim().is_empty())
                })
                .or_else(|| config.as_ref().and_then(|c| c.azure_tts_key.clone()))
                .context(
                    "Azure TTS requires a subscription key \
                     (set AZURE_TTS_KEY, pass --api-key, or set `azure_tts_key` in config)",
                )?;

            let region = args
                .azure_region
                .map(String::from)
                .or_else(|| {
                    std::env::var("AZURE_TTS_REGION")
                        .ok()
                        .filter(|r| !r.trim().is_empty())
                })
                .or_else(|| config.as_ref().and_then(|c| c.azure_tts_region.clone()))
                .context(
                    "Azure TTS requires a region \
                     (pass --azure-region, set AZURE_TTS_REGION, or set `azure_tts_region` in config)",
                )?;

            ResolvedProvider::Azure {
                subscription_key,
                region,
            }
        }
        other => bail!("unknown TTS provider '{other}' (expected: openai or azure)"),
    };

    // Azure ignores model/speed; drop them on the floor so the cache key
    // doesn't accidentally fork on config the provider will never send.
    let (model, speed) = match &provider {
        ResolvedProvider::Azure { .. } => (None, None),
        ResolvedProvider::OpenAi { .. } => (tts_model, args.speed),
    };

    Ok(TtsRuntime {
        provider,
        voice,
        model,
        format,
        speed,
        batch_size: args.batch_size,
        retries: args.retries,
        force: args.force,
        dry_run: args.dry_run,
    })
}

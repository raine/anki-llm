use anyhow::{Context, Result, bail};

use crate::config::store::read_config;
use crate::template::frontmatter::{TtsSource, TtsSpec};

use super::provider::{AudioFormat, ProviderSelection};
use super::template::TemplateSource;

/// Typed provider identity for a resolved spec. Carries provider-specific
/// credentials and endpoint data in a shape that can't drop fields: Azure
/// cannot be built without a region or subscription key, and OpenAI cannot
/// silently inherit an Azure region.
#[derive(Debug, Clone)]
pub enum ResolvedProvider {
    OpenAi {
        api_key: Option<String>,
        api_base_url: Option<String>,
    },
    Azure {
        subscription_key: String,
        region: String,
    },
}

impl ResolvedProvider {
    pub fn id(&self) -> &'static str {
        match self {
            Self::OpenAi { .. } => "openai",
            Self::Azure { .. } => "azure",
        }
    }

    /// Endpoint identity used in cache-key derivation. Distinct from the
    /// full HTTP endpoint a provider actually POSTs to — we want this key
    /// to stay stable across minor endpoint-path changes but still flip
    /// when the user points at a different region / base URL.
    pub fn endpoint_identity(&self) -> Option<String> {
        match self {
            Self::OpenAi { api_base_url, .. } => api_base_url.clone(),
            Self::Azure { region, .. } => Some(super::provider::azure::endpoint_identity(region)),
        }
    }

    pub fn into_selection(self) -> ProviderSelection {
        match self {
            Self::OpenAi {
                api_key,
                api_base_url,
            } => ProviderSelection::OpenAi {
                api_key,
                api_base_url,
            },
            Self::Azure {
                subscription_key,
                region,
            } => ProviderSelection::Azure {
                subscription_key,
                region,
            },
        }
    }
}

/// Fully-resolved TTS settings derived from a frontmatter `tts:` block,
/// merged only with env/CLI runtime secrets — NOT with `AppConfig.tts_*`.
/// Deck-design fields (voice / model / format / speed / target / source)
/// all come from the YAML; `tts_*` in `AppConfig` remains a legacy-mode
/// concern.
#[derive(Debug)]
pub struct ResolvedTtsSpec {
    pub provider: ResolvedProvider,
    pub voice: String,
    pub model: Option<String>,
    pub format: AudioFormat,
    pub speed: Option<f32>,
    pub target: String,
    pub source: TemplateSource,
    pub batch_size: u32,
    pub retries: u32,
    pub force: bool,
    pub dry_run: bool,
}

pub struct CliOverrides<'a> {
    pub api_key: Option<&'a str>,
    pub api_base_url: Option<&'a str>,
    pub azure_region: Option<&'a str>,
    pub batch_size: u32,
    pub retries: u32,
    pub force: bool,
    pub dry_run: bool,
}

pub fn resolve(spec: &TtsSpec, overrides: &CliOverrides) -> Result<ResolvedTtsSpec> {
    let config = read_config().ok();

    let provider_name = spec
        .provider
        .clone()
        .unwrap_or_else(|| "openai".to_string());

    let format_str = spec.format.clone().unwrap_or_else(|| "mp3".to_string());
    let format = AudioFormat::parse(&format_str).map_err(anyhow::Error::msg)?;

    let source = source_from(&spec.source).context("invalid tts.source")?;

    let provider = match provider_name.as_str() {
        "openai" => {
            let api_base_url = overrides
                .api_base_url
                .map(String::from)
                .or_else(|| config.as_ref().and_then(|c| c.api_base_url.clone()));

            // CLI > env > config. For OpenAI the env fallback is
            // ANKI_LLM_API_KEY > OPENAI_API_KEY (matches legacy behavior).
            let api_key = overrides.api_key.map(String::from).or_else(|| {
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
            // CLI > env > config.
            let subscription_key = overrides
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

            // Region precedence: YAML > CLI --azure-region > env > config.
            // YAML wins because it's deck-design, but we still let the CLI
            // flag fill in when the YAML is silent (it's optional when
            // provider != azure, but the CLI flag still matters for flag
            // mode).
            let region = spec
                .region
                .clone()
                .or_else(|| overrides.azure_region.map(String::from))
                .or_else(|| {
                    std::env::var("AZURE_TTS_REGION")
                        .ok()
                        .filter(|r| !r.trim().is_empty())
                })
                .or_else(|| config.as_ref().and_then(|c| c.azure_tts_region.clone()))
                .context(
                    "Azure TTS requires a region \
                     (set tts.region in YAML, pass --azure-region, \
                      set AZURE_TTS_REGION, or set `azure_tts_region` in config)",
                )?;

            ResolvedProvider::Azure {
                subscription_key,
                region,
            }
        }
        other => bail!("unknown TTS provider '{other}' (expected: openai or azure)"),
    };

    // Azure ignores model/speed; the parser already rejected them in the
    // YAML, but legacy flag mode can still pass them in. Drop them on the
    // floor here rather than forwarding them into the resolved spec.
    let (model, speed) = match &provider {
        ResolvedProvider::Azure { .. } => (None, None),
        ResolvedProvider::OpenAi { .. } => (spec.model.clone(), spec.speed),
    };

    Ok(ResolvedTtsSpec {
        provider,
        voice: spec.voice.clone(),
        model,
        format,
        speed,
        target: spec.target.clone(),
        source,
        batch_size: overrides.batch_size,
        retries: overrides.retries,
        force: overrides.force,
        dry_run: overrides.dry_run,
    })
}

fn source_from(src: &TtsSource) -> Result<TemplateSource> {
    match (&src.field, &src.template) {
        (Some(field), None) => Ok(TemplateSource::field(field.clone())),
        (None, Some(template)) => Ok(TemplateSource::inline(
            "tts.source.template".to_string(),
            template.clone(),
        )),
        _ => bail!("tts.source must set exactly one of `field` or `template`"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(field: Option<&str>, template: Option<&str>) -> TtsSpec {
        TtsSpec {
            target: "Audio".into(),
            source: TtsSource {
                field: field.map(String::from),
                template: template.map(String::from),
            },
            voice: "alloy".into(),
            provider: None,
            region: None,
            model: None,
            format: None,
            speed: None,
        }
    }

    fn overrides() -> CliOverrides<'static> {
        CliOverrides {
            api_key: Some("sk-test"),
            api_base_url: None,
            azure_region: None,
            batch_size: 5,
            retries: 3,
            force: false,
            dry_run: false,
        }
    }

    #[test]
    fn resolves_defaults() {
        let r = resolve(&spec(Some("front"), None), &overrides()).unwrap();
        assert_eq!(r.provider.id(), "openai");
        assert_eq!(r.format, AudioFormat::Mp3);
        assert_eq!(r.voice, "alloy");
        assert_eq!(r.target, "Audio");
        assert!(r.model.is_none());
        assert!(matches!(r.source, TemplateSource::Field(ref s) if s == "front"));
    }

    #[test]
    fn template_source_becomes_inline() {
        let r = resolve(&spec(None, Some("{front}")), &overrides()).unwrap();
        match r.source {
            TemplateSource::Inline { label, contents } => {
                assert_eq!(label, "tts.source.template");
                assert_eq!(contents, "{front}");
            }
            _ => panic!("expected Inline variant"),
        }
    }

    #[test]
    fn yaml_provider_overrides_default() {
        let mut s = spec(Some("front"), None);
        s.provider = Some("openai".into());
        s.model = Some("gpt-4o-mini-tts".into());
        s.speed = Some(1.25);
        let r = resolve(&s, &overrides()).unwrap();
        assert_eq!(r.model.as_deref(), Some("gpt-4o-mini-tts"));
        assert_eq!(r.speed, Some(1.25));
    }

    #[test]
    fn unknown_format_errors() {
        let mut s = spec(Some("front"), None);
        s.format = Some("flac".into());
        assert!(resolve(&s, &overrides()).is_err());
    }

    #[test]
    fn openai_without_key_errors() {
        let overrides = CliOverrides {
            api_key: None,
            api_base_url: None,
            azure_region: None,
            batch_size: 5,
            retries: 3,
            force: false,
            dry_run: false,
        };
        if std::env::var("OPENAI_API_KEY").is_ok() || std::env::var("ANKI_LLM_API_KEY").is_ok() {
            return;
        }
        match resolve(&spec(Some("front"), None), &overrides) {
            Ok(_) => panic!("expected error, got Ok"),
            Err(e) => assert!(e.to_string().contains("API key")),
        }
    }

    #[test]
    fn azure_resolves_with_cli_flags() {
        let mut s = spec(Some("front"), None);
        s.provider = Some("azure".into());
        s.region = Some("eastus".into());
        s.voice = "ja-JP-MasaruMultilingualNeural".into();
        let ov = CliOverrides {
            api_key: Some("fake-azure-key"),
            api_base_url: None,
            azure_region: None,
            batch_size: 5,
            retries: 3,
            force: false,
            dry_run: false,
        };
        let r = resolve(&s, &ov).unwrap();
        assert_eq!(r.provider.id(), "azure");
        match &r.provider {
            ResolvedProvider::Azure {
                subscription_key,
                region,
            } => {
                assert_eq!(subscription_key, "fake-azure-key");
                assert_eq!(region, "eastus");
            }
            other => panic!("expected Azure, got {other:?}"),
        }
        // Azure drops model/speed unconditionally.
        assert!(r.model.is_none());
        assert!(r.speed.is_none());
    }

    #[test]
    fn azure_missing_key_errors() {
        // Skip if AZURE_TTS_KEY is set in the test environment.
        if std::env::var("AZURE_TTS_KEY").is_ok() {
            return;
        }
        let mut s = spec(Some("front"), None);
        s.provider = Some("azure".into());
        s.region = Some("eastus".into());
        let ov = CliOverrides {
            api_key: None,
            api_base_url: None,
            azure_region: None,
            batch_size: 5,
            retries: 3,
            force: false,
            dry_run: false,
        };
        let err = resolve(&s, &ov).unwrap_err();
        assert!(err.to_string().contains("subscription key"));
    }
}

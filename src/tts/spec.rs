use anyhow::{Context, Result, bail};

use crate::config::store::read_config;
use crate::template::frontmatter::{TtsSource, TtsSpec};

use super::provider::AudioFormat;
use super::template::TemplateSource;

/// Fully-resolved TTS settings derived from a frontmatter `tts:` block,
/// merged only with env/CLI runtime secrets — NOT with `AppConfig.tts_*`.
/// Deck-design fields (voice / model / format / speed / target / source)
/// all come from the YAML; `tts_*` in `AppConfig` remains a legacy-mode
/// concern.
pub struct ResolvedTtsSpec {
    pub provider: String,
    pub voice: String,
    pub model: Option<String>,
    pub format: AudioFormat,
    pub speed: Option<f32>,
    pub api_key: Option<String>,
    pub api_base_url: Option<String>,
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
    pub batch_size: u32,
    pub retries: u32,
    pub force: bool,
    pub dry_run: bool,
}

pub fn resolve(spec: &TtsSpec, overrides: &CliOverrides) -> Result<ResolvedTtsSpec> {
    let config = read_config().ok();

    let provider = spec
        .provider
        .clone()
        .unwrap_or_else(|| "openai".to_string());
    let model = spec.model.clone();
    let format_str = spec.format.clone().unwrap_or_else(|| "mp3".to_string());
    let format = AudioFormat::parse(&format_str).map_err(anyhow::Error::msg)?;

    let api_base_url = overrides
        .api_base_url
        .map(String::from)
        .or_else(|| config.as_ref().and_then(|c| c.api_base_url.clone()));

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

    if provider == "openai" && api_key.is_none() && api_base_url.is_none() {
        bail!(
            "OpenAI TTS requires an API key (set OPENAI_API_KEY or ANKI_LLM_API_KEY, or pass --api-key)"
        );
    }

    let source = source_from(&spec.source).context("invalid tts.source")?;

    Ok(ResolvedTtsSpec {
        provider,
        voice: spec.voice.clone(),
        model,
        format,
        speed: spec.speed,
        api_key,
        api_base_url,
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
            model: None,
            format: None,
            speed: None,
        }
    }

    fn overrides() -> CliOverrides<'static> {
        CliOverrides {
            api_key: Some("sk-test"),
            api_base_url: None,
            batch_size: 5,
            retries: 3,
            force: false,
            dry_run: false,
        }
    }

    #[test]
    fn resolves_defaults() {
        let r = resolve(&spec(Some("front"), None), &overrides()).unwrap();
        assert_eq!(r.provider, "openai");
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
            batch_size: 5,
            retries: 3,
            force: false,
            dry_run: false,
        };
        // This test assumes the environment does not carry an OPENAI_API_KEY
        // / ANKI_LLM_API_KEY. If it does, skip the assertion — `resolve` is
        // doing the right thing using real env vars.
        if std::env::var("OPENAI_API_KEY").is_ok()
            || std::env::var("ANKI_LLM_API_KEY").is_ok()
        {
            return;
        }
        match resolve(&spec(Some("front"), None), &overrides) {
            Ok(_) => panic!("expected error, got Ok"),
            Err(e) => assert!(e.to_string().contains("API key")),
        }
    }
}

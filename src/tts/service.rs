use std::cell::OnceCell;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use indexmap::IndexMap;
use serde_json::Value;

use crate::anki::client::AnkiClient;
use crate::data::Row;
use crate::template::frontmatter::TtsSpec;

use super::cache::TtsCache;
use super::error::TtsError;
use super::ir::parse_furigana;
use super::media::{AnkiMediaStore, format_sound_tag};
use super::provider::{
    AudioFormat, SynthesisRequest, TextFormat, TtsProvider, build as build_provider,
};
use super::render::{render_plain_text, render_ssml};
use super::spec::{CliOverrides, resolve as resolve_tts_spec};
use super::template::TemplateSource;
use super::text::strip_annotations;

/// A fully-prepared synthesis job. Holds the provider-ready payload, the
/// content-addressed cache filename, the sound tag to stamp onto the Anki
/// field, and the local cache path. Callers use this as both a cache-key
/// identity and a way to decide whether a card's previously-previewed
/// audio is still current.
#[derive(Debug, Clone)]
pub struct PreparedSynthesis {
    pub request: SynthesisRequest,
    pub filename: String,
    pub sound_tag: String,
    pub cache_path: PathBuf,
    /// Character count of the spoken text (the plain-text rendering of
    /// the IR). Used by the batch engine for progress-bar metrics.
    pub spoken_chars: u64,
}

/// Reusable synthesis core shared by the standalone `tts` batch command,
/// the `generate` TUI preview hotkey, and the import-time finalizer.
///
/// `TtsService` owns the provider, cache, source template, target field,
/// and endpoint identity for one deck/run. It does NOT own the Anki media
/// store — upload happens through `ensure_uploaded`, which takes the
/// store as a parameter so the same service instance can be shared across
/// the standalone command (its own `AnkiMediaStore`) and the generate
/// worker (a separately-owned one).
pub struct TtsService {
    provider: Arc<dyn TtsProvider>,
    cache: Arc<TtsCache>,
    source: Arc<TemplateSource>,
    target_field: String,
    voice: String,
    model: Option<String>,
    format: AudioFormat,
    speed: Option<f32>,
    endpoint: Option<String>,
}

pub struct TtsServiceConfig {
    pub provider: Arc<dyn TtsProvider>,
    pub cache: Arc<TtsCache>,
    pub source: Arc<TemplateSource>,
    pub target_field: String,
    pub voice: String,
    pub model: Option<String>,
    pub format: AudioFormat,
    pub speed: Option<f32>,
    pub endpoint: Option<String>,
}

impl TtsService {
    pub fn new(cfg: TtsServiceConfig) -> Self {
        Self {
            provider: cfg.provider,
            cache: cfg.cache,
            source: cfg.source,
            target_field: cfg.target_field,
            voice: cfg.voice,
            model: cfg.model,
            format: cfg.format,
            speed: cfg.speed,
            endpoint: cfg.endpoint,
        }
    }

    pub fn target_field(&self) -> &str {
        &self.target_field
    }

    pub fn provider_id(&self) -> &'static str {
        self.provider.id()
    }

    pub fn voice(&self) -> &str {
        &self.voice
    }

    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    /// Build a `PreparedSynthesis` from an LLM-keyed eval row. This is the
    /// core preparation path — template expansion, normalization, IR
    /// parsing, provider-specific rendering, and cache-identity derivation.
    /// No network IO, no disk IO beyond what `TtsCache::filename` needs.
    pub fn prepare_from_row(&self, eval_row: &Row) -> Result<PreparedSynthesis> {
        let raw = self
            .source
            .expand(eval_row)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let stripped = super::text::normalize(&raw);
        if stripped.is_empty() {
            bail!("source text is empty after normalization");
        }

        let utterance = match parse_furigana(&stripped) {
            Ok(u) => u,
            Err(_) => {
                // Raw LLM output may contain brackets that are not valid
                // furigana annotations (e.g. `[N3]`, `英語[English]`). Strip
                // them and retry so preview still produces audio.
                let fallback = strip_annotations(&stripped);
                parse_furigana(&fallback).with_context(|| "failed to parse furigana")?
            }
        };
        if utterance.is_empty() {
            bail!("source text has no renderable content");
        }

        let text_format = self.provider.text_format();
        let payload = match text_format {
            TextFormat::PlainText => render_plain_text(&utterance),
            TextFormat::Ssml => render_ssml(&utterance, &self.voice),
        };
        let spoken_chars = render_plain_text(&utterance).chars().count() as u64;

        let request = SynthesisRequest {
            payload,
            provider_id: self.provider.id().to_string(),
            text_format,
            voice: self.voice.clone(),
            format: self.format,
            model: self.model.clone(),
            speed: self.speed,
            endpoint: self.endpoint.clone(),
        };

        let filename = TtsCache::filename(&request);
        let cache_path = self.cache.path_for(&request);
        let sound_tag = format_sound_tag(&filename);

        Ok(PreparedSynthesis {
            request,
            filename,
            sound_tag,
            cache_path,
            spoken_chars,
        })
    }

    /// Convenience wrapper around `prepare_from_row` that projects an
    /// Anki-keyed row through `field_map` into an LLM-keyed row first.
    /// This matches how the standalone `tts` command and the generate
    /// finalizer both see cards.
    pub fn prepare_from_anki_fields(
        &self,
        anki_fields: &IndexMap<String, String>,
        field_map: &IndexMap<String, String>,
    ) -> Result<PreparedSynthesis> {
        let eval_row = eval_row_from_anki_fields(anki_fields, field_map);
        self.prepare_from_row(&eval_row)
    }

    /// Ensure the cache has audio bytes for this prepared synthesis. On
    /// cache miss, calls the provider and stores the returned bytes. On
    /// cache hit, returns immediately without touching the network.
    /// Never uploads.
    pub fn ensure_cached(&self, prepared: &PreparedSynthesis) -> Result<Vec<u8>, TtsError> {
        if let Some(bytes) = self.cache.load(&prepared.request) {
            return Ok(bytes);
        }
        let bytes = self.provider.synthesize(&prepared.request)?;
        self.cache
            .store(&prepared.request, &bytes)
            .map_err(|e| TtsError::Permanent(format!("cache write failed: {e}")))?;
        Ok(bytes)
    }

    /// Upload audio bytes for this prepared synthesis to Anki via the
    /// given media store. Returns the filename AnkiConnect reports it
    /// stored under (may differ from `prepared.filename` on collision).
    pub fn ensure_uploaded(
        &self,
        prepared: &PreparedSynthesis,
        bytes: &[u8],
        media: &AnkiMediaStore,
    ) -> Result<String, TtsError> {
        media.ensure_uploaded(&prepared.filename, bytes)
    }
}

/// A `TtsService` bundled with its Anki media store. The generate
/// pipeline builds this once per session when `frontmatter.tts` is
/// present and shares it with both the preview path and the import-time
/// finalizer so they hit the same cache and the same upload-dedup map.
pub struct TtsBundle {
    pub service: Arc<TtsService>,
    pub media: Arc<AnkiMediaStore>,
}

/// Session-scoped TTS state: the spec from the prompt frontmatter plus a
/// lazily-initialized `TtsBundle`. The generate pipeline constructs this
/// at session startup when the prompt declares a `tts:` block, but
/// defers the actual bundle build (which resolves credentials via
/// `spec::resolve`) until the first preview or import-time finalization.
///
/// This matters because `--dry-run` and `--output` (export to YAML) —
/// and any session where the user hits Esc before submitting — never
/// need TTS. Eagerly building the bundle used to fail those runs with
/// `Azure TTS requires a subscription key` even though no synthesis
/// ever happens.
///
/// `std::cell::OnceCell` is single-threaded, which matches the
/// `PreparedSession` ownership model: it's created on the worker thread
/// (TUI mode) or the main thread (legacy mode) and never shared across
/// threads.
pub struct SessionTts {
    spec: TtsSpec,
    bundle: OnceCell<TtsBundle>,
}

impl SessionTts {
    pub fn new(spec: TtsSpec) -> Self {
        Self {
            spec,
            bundle: OnceCell::new(),
        }
    }

    /// Return the cached `TtsBundle`, building it on first call.
    ///
    /// Construction failures are NOT cached — a later retry after the
    /// user fixes their environment (e.g. exports `AZURE_TTS_KEY` and
    /// presses `p` again) will rebuild from scratch. Successful builds
    /// are cached for the lifetime of the session.
    pub fn bundle(&self) -> Result<&TtsBundle> {
        if let Some(b) = self.bundle.get() {
            return Ok(b);
        }
        let built = build_bundle(
            &self.spec,
            AnkiClient::new(),
            TtsBundleOptions { azure_region: None },
        )?;
        // Single-threaded OnceCell — nothing else can beat us to `set`.
        let _ = self.bundle.set(built);
        Ok(self
            .bundle
            .get()
            .expect("bundle was just set; get cannot fail"))
    }

    /// For tests only: inject a pre-built bundle so callers can exercise
    /// the lazy-resolution code path without touching real credentials.
    #[cfg(test)]
    pub fn with_bundle(spec: TtsSpec, bundle: TtsBundle) -> Self {
        let cell = OnceCell::new();
        let _ = cell.set(bundle);
        Self { spec, bundle: cell }
    }
}

/// Inputs needed to build a `TtsBundle` from a `TtsSpec`. Only the
/// Azure region is accepted here — `api_key` / `api_base_url` are
/// intentionally *not* plumbed through, because the one generate-side
/// caller that used to forward them was routing LLM transport flags
/// (\`--api-key\`, \`--api-base-url\`) into TTS, breaking setups that
/// pointed generate at OpenRouter / Ollama / local proxies. TTS
/// credentials now resolve exclusively through env vars and
/// `~/.config/anki-llm/config.json`'s `tts_*` fields via `spec::resolve`.
pub struct TtsBundleOptions<'a> {
    pub azure_region: Option<&'a str>,
}

/// Build a `TtsBundle` from a prompt-file `TtsSpec` plus an existing
/// `AnkiClient`. Re-uses `spec::resolve` for provider/env/config
/// fallbacks so generate and standalone `tts --prompt` land on identical
/// credentials.
pub fn build_bundle(
    spec: &TtsSpec,
    anki: AnkiClient,
    opts: TtsBundleOptions<'_>,
) -> Result<TtsBundle> {
    // `spec::resolve` needs a full `CliOverrides`; fill batch fields with
    // values that never drive generate's finalization (no --batch-size,
    // no --retries, no --force, no --dry-run here — those belong to the
    // standalone `tts` command, not to generate).
    let overrides = CliOverrides {
        api_key: None,
        api_base_url: None,
        azure_region: opts.azure_region,
        // Generate-side callers never forward AWS creds yet; they flow
        // through env/config via `spec::resolve` the same way Azure
        // does when `azure_region` is None.
        aws_access_key_id: None,
        aws_secret_access_key: None,
        aws_region: None,
        batch_size: 1,
        retries: 0,
        force: false,
        dry_run: false,
    };
    let resolved = resolve_tts_spec(spec, &overrides)?;

    let endpoint_identity = resolved.provider.endpoint_identity();
    let provider = build_provider(resolved.provider.clone().into_selection());

    let cache_dir = TtsCache::default_dir()
        .context("failed to locate cache directory (home dir unavailable)")?;
    let cache = Arc::new(TtsCache::new(cache_dir).context("failed to initialize TTS cache")?);
    let media = Arc::new(AnkiMediaStore::new(anki));

    let service = Arc::new(TtsService::new(TtsServiceConfig {
        provider,
        cache,
        source: Arc::new(resolved.source),
        target_field: resolved.target,
        voice: resolved.voice,
        model: resolved.model,
        format: resolved.format,
        speed: resolved.speed,
        endpoint: endpoint_identity,
    }));

    Ok(TtsBundle { service, media })
}

/// Project an Anki-keyed row (or field map) through `field_map` into an
/// LLM-keyed `Row` that `TemplateSource::expand` can operate on.
///
/// The generate pipeline uses `IndexMap<String, String>` for `anki_fields`
/// (and its raw sibling), while the batch path uses `serde_json::Value`-
/// valued rows. Both funnel through this projector — the output is the
/// same shape the standalone `tts` command's `build_eval_row` was already
/// producing.
pub fn eval_row_from_anki_fields(
    anki_fields: &IndexMap<String, String>,
    field_map: &IndexMap<String, String>,
) -> Row {
    let mut r: Row = IndexMap::new();
    for (llm_key, anki_name) in field_map {
        if let Some(value) = anki_fields.get(anki_name) {
            r.insert(llm_key.clone(), Value::String(value.clone()));
        }
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anki::client::AnkiClient;
    use crate::tts::cache::TtsCache;
    use crate::tts::error::TtsError;
    use crate::tts::provider::{AudioFormat, SynthesisRequest, TextFormat, TtsProvider};
    use crate::tts::template::TemplateSource;
    use std::sync::{Arc, Mutex};

    struct MockProvider {
        id: &'static str,
        text_format: TextFormat,
        calls: Mutex<Vec<SynthesisRequest>>,
    }

    impl MockProvider {
        fn new(id: &'static str, text_format: TextFormat) -> Self {
            Self {
                id,
                text_format,
                calls: Mutex::new(Vec::new()),
            }
        }

        fn call_count(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
    }

    impl TtsProvider for MockProvider {
        fn id(&self) -> &'static str {
            self.id
        }

        fn text_format(&self) -> TextFormat {
            self.text_format
        }

        fn synthesize(&self, req: &SynthesisRequest) -> Result<Vec<u8>, TtsError> {
            self.calls.lock().unwrap().push(req.clone());
            Ok(format!("audio:{}", req.payload).into_bytes())
        }
    }

    fn mk_service(
        provider: Arc<dyn TtsProvider>,
        cache: Arc<TtsCache>,
        source: TemplateSource,
        target_field: &str,
    ) -> TtsService {
        TtsService::new(TtsServiceConfig {
            provider,
            cache,
            source: Arc::new(source),
            target_field: target_field.to_string(),
            voice: "alloy".into(),
            model: None,
            format: AudioFormat::Mp3,
            speed: None,
            endpoint: None,
        })
    }

    fn map(pairs: &[(&str, &str)]) -> IndexMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn prepare_from_row_plain_text_flow() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = Arc::new(TtsCache::new(tmp.path().to_path_buf()).unwrap());
        let provider: Arc<dyn TtsProvider> =
            Arc::new(MockProvider::new("mock", TextFormat::PlainText));
        let svc = mk_service(
            provider,
            cache,
            TemplateSource::field("front".into()),
            "Audio",
        );

        let anki_fields = map(&[("Front", "日本語[にほんご]を"), ("Back", "japanese")]);
        let field_map = map(&[("front", "Front"), ("back", "Back")]);
        let prepared = svc
            .prepare_from_anki_fields(&anki_fields, &field_map)
            .unwrap();

        assert_eq!(prepared.request.payload, "にほんごを");
        assert_eq!(prepared.request.voice, "alloy");
        assert!(prepared.filename.starts_with("anki-llm-tts-"));
        assert!(prepared.filename.ends_with(".mp3"));
        assert_eq!(prepared.sound_tag, format!("[sound:{}]", prepared.filename));
        assert_eq!(prepared.spoken_chars, 5);
    }

    #[test]
    fn prepare_from_row_ssml_flow() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = Arc::new(TtsCache::new(tmp.path().to_path_buf()).unwrap());
        let provider: Arc<dyn TtsProvider> = Arc::new(MockProvider::new("mock", TextFormat::Ssml));
        let svc = mk_service(
            provider,
            cache,
            TemplateSource::field("front".into()),
            "Audio",
        );

        let anki_fields = map(&[("Front", "日本語[にほんご]")]);
        let field_map = map(&[("front", "Front")]);
        let prepared = svc
            .prepare_from_anki_fields(&anki_fields, &field_map)
            .unwrap();

        assert!(
            prepared
                .request
                .payload
                .contains("<sub alias=\"にほんご\">日本語</sub>")
        );
        assert!(prepared.request.payload.contains("<voice name=\"alloy\">"));
    }

    #[test]
    fn ensure_cached_hits_cache_on_second_call() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = Arc::new(TtsCache::new(tmp.path().to_path_buf()).unwrap());
        let mock = Arc::new(MockProvider::new("mock", TextFormat::PlainText));
        let provider: Arc<dyn TtsProvider> = mock.clone();
        let svc = mk_service(
            provider,
            cache,
            TemplateSource::field("front".into()),
            "Audio",
        );

        let anki_fields = map(&[("Front", "hello")]);
        let field_map = map(&[("front", "Front")]);
        let prepared = svc
            .prepare_from_anki_fields(&anki_fields, &field_map)
            .unwrap();

        let bytes1 = svc.ensure_cached(&prepared).unwrap();
        let bytes2 = svc.ensure_cached(&prepared).unwrap();
        assert_eq!(bytes1, bytes2);
        assert_eq!(bytes1, b"audio:hello");
        assert_eq!(mock.call_count(), 1, "second call should be a cache hit");
    }

    #[test]
    fn prepare_errors_on_empty_normalized_text() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = Arc::new(TtsCache::new(tmp.path().to_path_buf()).unwrap());
        let provider: Arc<dyn TtsProvider> =
            Arc::new(MockProvider::new("mock", TextFormat::PlainText));
        let svc = mk_service(
            provider,
            cache,
            TemplateSource::field("front".into()),
            "Audio",
        );

        let anki_fields = map(&[("Front", "<br/>   ")]);
        let field_map = map(&[("front", "Front")]);
        let err = svc
            .prepare_from_anki_fields(&anki_fields, &field_map)
            .unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn prepare_is_deterministic() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = Arc::new(TtsCache::new(tmp.path().to_path_buf()).unwrap());
        let provider: Arc<dyn TtsProvider> =
            Arc::new(MockProvider::new("mock", TextFormat::PlainText));
        let svc = mk_service(
            provider,
            cache,
            TemplateSource::field("front".into()),
            "Audio",
        );

        let anki_fields = map(&[("Front", "hello")]);
        let field_map = map(&[("front", "Front")]);
        let p1 = svc
            .prepare_from_anki_fields(&anki_fields, &field_map)
            .unwrap();
        let p2 = svc
            .prepare_from_anki_fields(&anki_fields, &field_map)
            .unwrap();
        assert_eq!(p1.filename, p2.filename);
        assert_eq!(p1.sound_tag, p2.sound_tag);
    }

    #[test]
    fn eval_row_projection_omits_unmapped_keys() {
        let anki_fields = map(&[("Front", "cat"), ("Back", "feline"), ("Audio", "")]);
        let fm = map(&[("front", "Front"), ("back", "Back")]);
        let row = eval_row_from_anki_fields(&anki_fields, &fm);
        assert_eq!(row.len(), 2);
        assert_eq!(row.get("front").and_then(|v| v.as_str()), Some("cat"));
        assert_eq!(row.get("back").and_then(|v| v.as_str()), Some("feline"));
        assert!(row.get("Front").is_none());
        assert!(row.get("Audio").is_none());
    }

    // Silence unused-import warning: AnkiClient is referenced only to keep
    // the test file scope linking against the same crate.
    #[allow(dead_code)]
    fn _touch(_: AnkiClient) {}

    // ----- SessionTts (lazy bundle) -----
    //
    // These tests pin the "do not resolve credentials until first use"
    // contract. Prior to the lazy refactor, sessions with a `tts:`
    // block in frontmatter would fail at startup — breaking
    // `--dry-run`, `--output`, and any run where the user never
    // pressed `p` — because `prepare_session` eagerly called
    // `build_bundle` → `spec::resolve`.

    use crate::template::frontmatter::{TtsSource, TtsSpec};

    fn bad_spec() -> TtsSpec {
        // `unknown-provider` fails deterministically inside
        // `spec::resolve` without depending on any env vars, so the
        // test is hermetic regardless of whether the developer has
        // AZURE_TTS_KEY / OPENAI_API_KEY set in their shell.
        TtsSpec {
            target: "Audio".into(),
            source: TtsSource {
                field: Some("front".into()),
                template: None,
            },
            voice: "alloy".into(),
            provider: Some("unknown-provider".into()),
            region: None,
            model: None,
            format: None,
            speed: None,
        }
    }

    #[test]
    fn session_tts_new_does_not_resolve_credentials() {
        // If `SessionTts::new` resolved credentials eagerly, this
        // `bad_spec` would panic here. The whole point of the lazy
        // refactor is that construction is side-effect free.
        let _ = SessionTts::new(bad_spec());
    }

    #[test]
    fn session_tts_bundle_errors_on_bad_spec_and_retries() {
        let session = SessionTts::new(bad_spec());

        let first_msg = match session.bundle() {
            Ok(_) => panic!("bad spec must surface a resolve error"),
            Err(e) => e.to_string(),
        };
        assert!(
            first_msg.contains("unknown TTS provider"),
            "expected provider error, got: {first_msg}"
        );

        // Errors are NOT cached — a second call re-runs resolution,
        // which matches the documented OnceCell semantics (failures
        // don't populate the cell, so the next caller retries).
        assert!(
            session.bundle().is_err(),
            "second call must also error (build_bundle is not cached on failure)"
        );
    }

    #[test]
    fn session_tts_bundle_caches_successful_build() {
        // Build a minimal bundle by hand and stash it via the test-only
        // `with_bundle` constructor. Subsequent `bundle()` calls must
        // return the same handle without re-running resolution —
        // verified by comparing pointer identity of the inner `Arc`s.
        let tmp = tempfile::tempdir().unwrap();
        let cache = Arc::new(TtsCache::new(tmp.path().to_path_buf()).unwrap());
        let provider: Arc<dyn TtsProvider> =
            Arc::new(MockProvider::new("mock", TextFormat::PlainText));
        let service = Arc::new(mk_service(
            provider,
            cache,
            TemplateSource::field("front".into()),
            "Audio",
        ));
        let media = Arc::new(AnkiMediaStore::new(AnkiClient::new()));
        let bundle = TtsBundle {
            service: service.clone(),
            media: media.clone(),
        };

        let session = SessionTts::with_bundle(bad_spec(), bundle);

        let first = session.bundle().expect("pre-seeded bundle must resolve");
        let second = session.bundle().expect("cached bundle must resolve");
        assert!(
            Arc::ptr_eq(&first.service, &second.service),
            "cached bundle must return the same service handle"
        );
        assert!(
            Arc::ptr_eq(&first.media, &second.media),
            "cached bundle must return the same media handle"
        );
        // Also verify the service pointer matches the one we injected,
        // proving `bundle()` did not rebuild from the spec.
        assert!(Arc::ptr_eq(&first.service, &service));
    }
}

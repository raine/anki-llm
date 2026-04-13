use std::sync::Arc;

use serde_json::Value;

use crate::batch::engine::ProcessFn;
use crate::batch::error::BatchError;
use crate::batch::report::ERROR_FIELD;
use crate::data::Row;

use super::cache::TtsCache;
use super::error::TtsError;
use super::media::{AnkiMediaStore, format_sound_tag};
use super::provider::{AudioFormat, SynthesisRequest, TtsProvider};
use super::template::TemplateSource;
use super::text::normalize;

/// Inputs for building the per-row TTS processing closure.
pub struct TtsProcessConfig {
    pub provider: Arc<dyn TtsProvider>,
    pub cache: Arc<TtsCache>,
    pub media: Arc<AnkiMediaStore>,
    pub source: Arc<TemplateSource>,
    pub target_field: String,
    pub voice: String,
    pub model: Option<String>,
    pub format: AudioFormat,
    pub speed: Option<f32>,
    /// API base URL passed along to the cache key. This mirrors the value
    /// the provider was constructed with; the provider itself still uses
    /// its own internal base URL to make the HTTP call.
    pub api_base_url: Option<String>,
}

fn tts_error_to_batch(e: TtsError) -> BatchError {
    match e {
        TtsError::Permanent(m) => BatchError::Fatal(m),
        TtsError::Transient(m) => BatchError::Processing(m),
    }
}

/// Build the row-processing closure used by the TTS deck/file commands.
///
/// For each row:
/// 1. Expand the source text (template or field reference).
/// 2. Normalize for synthesis + cache hashing.
/// 3. Hit the local disk cache; on miss, call the provider and cache the
///    returned bytes.
/// 4. Upload to Anki's media store via `AnkiMediaStore` (deduplicated per
///    run).
/// 5. Replace the target field with `[sound:<filename>]`.
pub fn build_tts_process_fn(cfg: TtsProcessConfig) -> ProcessFn {
    Arc::new(move |row: &Row| {
        let raw = cfg
            .source
            .expand(row)
            .map_err(|e| BatchError::Fatal(e.to_string()))?;
        let text = normalize(&raw);
        if text.is_empty() {
            return Err(BatchError::Fatal(
                "source text is empty after normalization".to_string(),
            ));
        }

        let req = SynthesisRequest {
            text: text.clone(),
            provider_id: cfg.provider.id().to_string(),
            voice: cfg.voice.clone(),
            format: cfg.format,
            model: cfg.model.clone(),
            speed: cfg.speed,
            api_base_url: cfg.api_base_url.clone(),
        };

        let bytes = if let Some(cached) = cfg.cache.load(&req) {
            cached
        } else {
            let bytes = cfg.provider.synthesize(&req).map_err(tts_error_to_batch)?;
            cfg.cache
                .store(&req, &bytes)
                .map_err(|e| BatchError::Processing(format!("cache write failed: {e}")))?;
            bytes
        };

        let filename = TtsCache::filename(&req);
        let stored = cfg
            .media
            .ensure_uploaded(&filename, &bytes)
            .map_err(tts_error_to_batch)?;

        let tag = format_sound_tag(&stored);
        let mut out = row.clone();
        out.insert(cfg.target_field.clone(), Value::String(tag));
        out.shift_remove(ERROR_FIELD);

        // "Input units" for the progress display = characters of normalized
        // spoken text. No output unit concept for TTS.
        let usage = Some((text.chars().count() as u64, 0u64));
        Ok((out, usage))
    })
}

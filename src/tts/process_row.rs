use std::sync::Arc;

use indexmap::IndexMap;
use serde_json::Value;

use crate::batch::engine::ProcessFn;
use crate::batch::error::BatchError;
use crate::batch::report::ERROR_FIELD;
use crate::data::Row;

use super::cache::TtsCache;
use super::error::TtsError;
use super::ir::parse_furigana;
use super::media::{AnkiMediaStore, format_sound_tag};
use super::provider::{AudioFormat, SynthesisRequest, TextFormat, TtsProvider};
use super::render::{render_plain_text, render_ssml};
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
    /// Endpoint identity for cache-key derivation. For OpenAI this is the
    /// `api_base_url`; for Azure it's the derived region endpoint. The
    /// provider itself still uses its own internal endpoint to make the
    /// HTTP call.
    pub endpoint: Option<String>,
    /// Prompt-mode field_map projection. When set, source-text expansion
    /// happens against a per-row eval context built only from field_map
    /// (LLM key → value of the corresponding Anki field), so YAML
    /// `tts.source` can reference field_map keys without colliding with
    /// the Anki-named keys on the persisted row that DeckWriter sends
    /// back to Anki.
    pub field_map: Option<IndexMap<String, String>>,
}

fn tts_error_to_batch(e: TtsError) -> BatchError {
    match e {
        TtsError::Permanent(m) => BatchError::Fatal(m),
        TtsError::Transient(m) => BatchError::Processing(m),
    }
}

/// Build the row that source-text expansion will see for a single TTS job.
///
/// In prompt mode, returns a fresh row keyed by `field_map` LLM keys (each
/// pointing at the value of the corresponding Anki field). In legacy mode
/// (`field_map` = None), returns the Anki-keyed row unchanged.
///
/// Crucially, this is *separate* from the row the batch writer sends back
/// to Anki — augmenting the persisted row would both (a) leak unknown
/// LLM-key fields into `updateNoteFields` and (b) collide with
/// `fill_template`'s case-insensitive lookup whenever an LLM key is just
/// the lowercase of its Anki field name (e.g. `front` and `Front`).
pub(super) fn build_eval_row(row: &Row, field_map: Option<&IndexMap<String, String>>) -> Row {
    if let Some(map) = field_map {
        let mut r: Row = IndexMap::new();
        for (llm_key, anki_name) in map {
            if let Some(value) = row.get(anki_name).cloned() {
                r.insert(llm_key.clone(), value);
            }
        }
        r
    } else {
        row.clone()
    }
}

/// Build the row-processing closure used by the TTS batch flow.
///
/// For each row:
/// 1. Project the row into a source-expansion context (see `build_eval_row`).
/// 2. Expand the source text (template or field reference).
/// 3. Strip HTML / cloze / sound tags / entities (see `text::normalize`).
/// 4. Parse the result into an `Utterance` (Han-cluster furigana IR).
/// 5. Render to the provider's native format (plain text or SSML).
/// 6. Hit the local disk cache keyed on the prepared payload; on miss,
///    call the provider and cache the returned bytes.
/// 7. Upload to Anki's media store via `AnkiMediaStore` (deduplicated per run).
/// 8. Replace the target field on the original row with `[sound:<filename>]`.
pub fn build_tts_process_fn(cfg: TtsProcessConfig) -> ProcessFn {
    Arc::new(move |row: &Row| {
        let eval_row = build_eval_row(row, cfg.field_map.as_ref());

        let raw = cfg
            .source
            .expand(&eval_row)
            .map_err(|e| BatchError::Fatal(e.to_string()))?;
        let stripped = normalize(&raw);
        if stripped.is_empty() {
            return Err(BatchError::Fatal(
                "source text is empty after normalization".to_string(),
            ));
        }

        let utterance = parse_furigana(&stripped)
            .map_err(|e| BatchError::Fatal(format!("failed to parse furigana: {e}")))?;
        if utterance.is_empty() {
            return Err(BatchError::Fatal(
                "source text has no renderable content".to_string(),
            ));
        }

        let text_format = cfg.provider.text_format();
        let payload = match text_format {
            TextFormat::PlainText => render_plain_text(&utterance),
            TextFormat::Ssml => render_ssml(&utterance, &cfg.voice),
        };
        // Character count for the progress display uses the spoken text
        // length, not the full SSML envelope length. The IR's plain-text
        // rendering is the closest proxy.
        let spoken_chars = render_plain_text(&utterance).chars().count() as u64;

        let req = SynthesisRequest {
            payload,
            provider_id: cfg.provider.id().to_string(),
            text_format,
            voice: cfg.voice.clone(),
            format: cfg.format,
            model: cfg.model.clone(),
            speed: cfg.speed,
            endpoint: cfg.endpoint.clone(),
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

        let usage = Some((spoken_chars, 0u64));
        Ok((out, usage))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::template::fill_template;
    use serde_json::Value;

    fn anki_row(pairs: &[(&str, &str)]) -> Row {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
            .collect()
    }

    fn map(pairs: &[(&str, &str)]) -> IndexMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn legacy_mode_passes_row_through() {
        let row = anki_row(&[("Front", "cat"), ("Back", "feline")]);
        let eval = build_eval_row(&row, None);
        assert_eq!(eval.len(), 2);
        assert_eq!(eval.get("Front").and_then(|v| v.as_str()), Some("cat"));
    }

    #[test]
    fn prompt_mode_projects_field_map_keys_only() {
        let row = anki_row(&[("Front", "cat"), ("Back", "feline"), ("Audio", "")]);
        let fm = map(&[("front", "Front"), ("back", "Back")]);
        let eval = build_eval_row(&row, Some(&fm));
        assert_eq!(eval.len(), 2);
        assert_eq!(eval.get("front").and_then(|v| v.as_str()), Some("cat"));
        assert_eq!(eval.get("back").and_then(|v| v.as_str()), Some("feline"));
        // The Anki-name keys must NOT be in the eval row — that's what
        // would otherwise collide with fill_template's case-insensitive
        // lookup.
        assert!(eval.get("Front").is_none());
        assert!(eval.get("Audio").is_none());
    }

    #[test]
    fn prompt_mode_template_expansion_no_ambiguity_collision() {
        // The classic case: field_map is { front -> Front }. The persisted
        // row has `Front`. If the eval row contained both `Front` and
        // `front`, fill_template would return AmbiguousKeys. The
        // projection only keeps LLM keys, so this works.
        let row = anki_row(&[("Front", "cat")]);
        let fm = map(&[("front", "Front")]);
        let eval = build_eval_row(&row, Some(&fm));
        let result = fill_template("{front}", &eval).unwrap();
        assert_eq!(result, "cat");
    }

    #[test]
    fn prompt_mode_handles_missing_anki_field_gracefully() {
        // If the field_map references an Anki field that's missing from
        // the row (e.g., empty note), the eval row simply omits it. The
        // template's missing-placeholder error is then surfaced by
        // fill_template, not by the projector.
        let row = anki_row(&[("Front", "cat")]);
        let fm = map(&[("front", "Front"), ("back", "Back")]);
        let eval = build_eval_row(&row, Some(&fm));
        assert_eq!(eval.len(), 1);
        assert!(eval.get("front").is_some());
        assert!(eval.get("back").is_none());
    }
}

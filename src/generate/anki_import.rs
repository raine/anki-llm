use anyhow::{Context, Result};

use crate::anki::client::AnkiClient;
use crate::anki::schema::AddNoteParams;
use crate::style::style;
use crate::template::frontmatter::Frontmatter;
use crate::tts::media::{AnkiMediaStore, format_sound_tag};
use crate::tts::service::TtsService;

use super::cards::ValidatedCard;

pub struct ImportResult {
    pub successes: usize,
    pub failures: usize,
    /// Note IDs of successfully added notes.
    pub note_ids: Vec<i64>,
}

/// TTS finalization hook for `import_cards_to_anki`. When present, the
/// importer calls `finalize_tts` on the mutable card slice just before
/// building the `AddNoteParams` vector. This keeps audio out of Anki's
/// media store until the user has actually confirmed an import — a
/// cancelled selection never leaks orphan uploads.
pub struct TtsFinalize<'a> {
    pub service: &'a TtsService,
    pub media: &'a AnkiMediaStore,
}

/// Add cards to Anki as new notes. If `tts` is `Some`, run the TTS
/// finalizer against the cards (synthesize-on-miss, upload, rewrite the
/// target field) before `add_notes` is called.
///
/// Mutates `cards` in place: TTS finalization writes the resulting
/// `[sound:...]` tags into both `anki_fields[target]` and
/// `raw_anki_fields[target]` so the post-import "Done" view shows the
/// same data that was sent to `addNotes`.
pub fn import_cards_to_anki(
    cards: &mut [ValidatedCard],
    frontmatter: &Frontmatter,
    anki: &AnkiClient,
    tts: Option<TtsFinalize<'_>>,
    on_log: &dyn Fn(&str),
) -> Result<ImportResult, anyhow::Error> {
    if cards.is_empty() {
        return Ok(ImportResult {
            successes: 0,
            failures: 0,
            note_ids: Vec::new(),
        });
    }

    if let Some(finalizer) = tts {
        finalize_tts(cards, frontmatter, finalizer, on_log)?;
    }

    on_log(&format!("Adding {} card(s) to Anki...", cards.len()));

    let notes: Vec<AddNoteParams> = cards
        .iter()
        .map(|card| AddNoteParams {
            deck_name: frontmatter.deck.clone(),
            model_name: frontmatter.note_type.clone(),
            fields: card.anki_fields.clone(),
            tags: vec!["anki-llm-generate".into()],
        })
        .collect();

    let results = anki.add_notes(&notes)?;
    let note_ids: Vec<i64> = results.iter().filter_map(|r| *r).collect();
    let successes = note_ids.len();
    let failures = results.len() - successes;

    Ok(ImportResult {
        successes,
        failures,
        note_ids,
    })
}

/// Synthesize + upload audio for each card that the frontmatter's `tts:`
/// block targets, then rewrite `card.anki_fields[target]` to the
/// resulting `[sound:<stored>]` tag. Uses `raw_anki_fields` as the text
/// source (pre-HTML-sanitization, which is what TTS text normalization
/// needs) and projects through `frontmatter.field_map` so YAML
/// `tts.source` can reference field_map keys.
///
/// On any error (parse failure, synth failure, upload failure), aborts
/// the whole import with that error. Import-without-audio is the wrong
/// default when `tts:` is part of the deck design — a silently missing
/// sound tag would be much worse than a clear finalization failure.
pub fn finalize_tts(
    cards: &mut [ValidatedCard],
    frontmatter: &Frontmatter,
    finalizer: TtsFinalize<'_>,
    on_log: &dyn Fn(&str),
) -> Result<()> {
    let TtsFinalize { service, media } = finalizer;

    let tts_target = service.target_field().to_string();
    on_log(&format!(
        "Finalizing TTS audio for {} card(s)...",
        cards.len()
    ));

    for (i, card) in cards.iter_mut().enumerate() {
        // Respect any existing target-field content: the user may have
        // typed a manual `[sound:...]` reference in `$EDITOR` or the LLM
        // may have populated the field itself. Synthesizing over it would
        // silently destroy their work. The standalone `anki-llm tts`
        // command applies the same guard via `field_has_sound_tag` by
        // default; there's no `--tts-force` flag on generate yet.
        let existing = card
            .anki_fields
            .get(&tts_target)
            .map(String::as_str)
            .unwrap_or("");
        if !existing.trim().is_empty() {
            on_log(&format!(
                "card #{}: skipping TTS (target field already populated)",
                i + 1
            ));
            continue;
        }

        let prepared = service
            .prepare_from_anki_fields(&card.raw_anki_fields, &frontmatter.field_map)
            .with_context(|| format!("card #{}: TTS preparation failed", i + 1))?;

        let bytes = service
            .ensure_cached(&prepared)
            .map_err(|e| anyhow::anyhow!("card #{}: TTS synthesis failed: {}", i + 1, e))?;

        let stored = service
            .ensure_uploaded(&prepared, &bytes, media)
            .map_err(|e| anyhow::anyhow!("card #{}: TTS upload failed: {}", i + 1, e))?;

        let tag = format_sound_tag(&stored);
        card.anki_fields.insert(tts_target.clone(), tag.clone());
        card.raw_anki_fields.insert(tts_target.clone(), tag);
    }
    Ok(())
}

/// Print import results to stderr (legacy mode).
pub fn report_import_result(result: &ImportResult, deck_name: &str) {
    let s = style();
    if result.failures > 0 {
        eprintln!(
            "\n  {} card(s) added, {} failed",
            result.successes,
            s.error_text(result.failures)
        );
        eprintln!(
            "  {}",
            s.muted("Some cards may have been duplicates or had invalid field values.")
        );
    } else if result.successes > 0 {
        eprintln!(
            "\n  {} {} note(s) to {}",
            s.success("Added"),
            result.successes,
            s.cyan(format!("\"{deck_name}\""))
        );
    } else {
        eprintln!("\n  No cards were added to Anki.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anki::client::AnkiClient;
    use crate::generate::cards::ValidatedCard;
    use crate::template::frontmatter::{TtsSource, TtsSpec};
    use crate::tts::cache::TtsCache;
    use crate::tts::error::TtsError;
    use crate::tts::provider::{AudioFormat, SynthesisRequest, TextFormat, TtsProvider};
    use crate::tts::service::{TtsService, TtsServiceConfig};
    use crate::tts::template::TemplateSource;
    use indexmap::IndexMap;
    use std::sync::{Arc, Mutex};

    struct MockProvider {
        calls: Mutex<Vec<SynthesisRequest>>,
    }

    impl MockProvider {
        fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl TtsProvider for MockProvider {
        fn id(&self) -> &'static str {
            "mock"
        }

        fn text_format(&self) -> TextFormat {
            TextFormat::PlainText
        }

        fn synthesize(&self, req: &SynthesisRequest) -> Result<Vec<u8>, TtsError> {
            self.calls.lock().unwrap().push(req.clone());
            Ok(format!("audio:{}", req.payload).into_bytes())
        }
    }

    fn mk_service(
        provider: Arc<dyn TtsProvider>,
        cache: Arc<TtsCache>,
        target: &str,
    ) -> Arc<TtsService> {
        Arc::new(TtsService::new(TtsServiceConfig {
            provider,
            cache,
            source: Arc::new(TemplateSource::field("front".into())),
            target_field: target.to_string(),
            voice: "alloy".into(),
            model: None,
            format: AudioFormat::Mp3,
            speed: None,
            endpoint: None,
        }))
    }

    fn mk_frontmatter() -> Frontmatter {
        let mut field_map = IndexMap::new();
        field_map.insert("front".to_string(), "Front".to_string());
        field_map.insert("back".to_string(), "Back".to_string());
        Frontmatter {
            title: None,
            description: None,
            deck: "Test".into(),
            note_type: "Basic".into(),
            field_map,
            processing: None,
            tts: Some(TtsSpec {
                target: "Audio".into(),
                source: TtsSource {
                    field: Some("front".into()),
                    template: None,
                },
                voice: "alloy".into(),
                provider: None,
                region: None,
                model: None,
                format: None,
                speed: None,
            }),
        }
    }

    fn mk_card(front_raw: &str) -> ValidatedCard {
        use std::collections::HashMap;
        let mut fields: HashMap<String, String> = HashMap::new();
        fields.insert("front".into(), front_raw.to_string());
        let mut anki_fields: IndexMap<String, String> = IndexMap::new();
        anki_fields.insert("Front".into(), front_raw.to_string());
        anki_fields.insert("Back".into(), "gloss".into());
        anki_fields.insert("Audio".into(), String::new());
        let mut raw_anki_fields = anki_fields.clone();
        raw_anki_fields.insert("Front".into(), front_raw.to_string());
        ValidatedCard {
            card_id: crate::generate::cards::next_card_id(),
            fields,
            anki_fields,
            raw_anki_fields,
            is_duplicate: false,
            duplicate_note_id: None,
            duplicate_fields: None,
            flags: Vec::new(),
            model: "test".into(),
        }
    }

    struct NoopMedia;

    // Minimal fake of `AnkiMediaStore` for finalize_tts tests. The real
    // store's `ensure_uploaded` hits AnkiConnect; we mask that by
    // short-circuiting through a local subclass that returns the
    // requested filename verbatim without touching the network.
    //
    // We reach this by constructing an `AnkiMediaStore` wrapping a
    // `AnkiClient` whose HTTP calls we stub out via env guard — simpler
    // to bypass the store entirely and test `finalize_tts` against a
    // small in-crate shim. But `finalize_tts` takes `&AnkiMediaStore`
    // concretely, so instead we add an integration-ish test that
    // exercises `prepare + ensure_cached` directly and stops short of
    // the upload step.
    #[allow(dead_code)]
    fn _noop(_: NoopMedia) {}

    #[test]
    fn finalize_cache_hit_when_already_previewed() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = Arc::new(TtsCache::new(tmp.path().to_path_buf()).unwrap());
        let mock = Arc::new(MockProvider::new());
        let provider: Arc<dyn TtsProvider> = mock.clone();
        let service = mk_service(provider, cache, "Audio");

        let frontmatter = mk_frontmatter();
        let card = mk_card("日本語[にほんご]を");

        // Simulate preview: prepare + ensure_cached.
        let prepared = service
            .prepare_from_anki_fields(&card.raw_anki_fields, &frontmatter.field_map)
            .unwrap();
        let _ = service.ensure_cached(&prepared).unwrap();
        assert_eq!(
            mock.calls.lock().unwrap().len(),
            1,
            "preview should hit the provider once"
        );

        // Now re-prepare + ensure_cached against the same card state:
        // filename must match (content-addressed) and provider must NOT
        // be called a second time.
        let prepared2 = service
            .prepare_from_anki_fields(&card.raw_anki_fields, &frontmatter.field_map)
            .unwrap();
        assert_eq!(prepared.filename, prepared2.filename);
        let _ = service.ensure_cached(&prepared2).unwrap();
        assert_eq!(
            mock.calls.lock().unwrap().len(),
            1,
            "finalization after preview must hit the cache, not the provider"
        );
    }

    #[test]
    fn finalize_skips_when_target_field_already_populated() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = Arc::new(TtsCache::new(tmp.path().to_path_buf()).unwrap());
        let mock = Arc::new(MockProvider::new());
        let provider: Arc<dyn TtsProvider> = mock.clone();
        let service = mk_service(provider, cache, "Audio");

        let frontmatter = mk_frontmatter();
        let mut card = mk_card("日本語[にほんご]を");
        // Simulate a user-authored or LLM-authored sound tag sitting in
        // the target field before finalize runs.
        let preexisting = "[sound:user-recorded.mp3]".to_string();
        card.anki_fields.insert("Audio".into(), preexisting.clone());
        card.raw_anki_fields
            .insert("Audio".into(), preexisting.clone());

        // We never expect `ensure_uploaded` to be called — the skip guard
        // must short-circuit before touching the media store — so it's
        // safe to hand finalize_tts a store wrapping a default client
        // that would normally hit AnkiConnect.
        let media = AnkiMediaStore::new(AnkiClient::new());
        let finalizer = TtsFinalize {
            service: &service,
            media: &media,
        };

        let mut cards = vec![card];
        finalize_tts(&mut cards, &frontmatter, finalizer, &|_| {}).unwrap();

        assert_eq!(
            mock.calls.lock().unwrap().len(),
            0,
            "populated target field must not trigger synthesis"
        );
        assert_eq!(
            cards[0].anki_fields.get("Audio").map(String::as_str),
            Some(preexisting.as_str()),
            "pre-existing Audio field must survive finalization untouched"
        );
        assert_eq!(
            cards[0].raw_anki_fields.get("Audio").map(String::as_str),
            Some(preexisting.as_str()),
            "raw Audio field must also survive untouched"
        );
    }

    #[test]
    fn finalize_stale_after_edit_resynthesizes() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = Arc::new(TtsCache::new(tmp.path().to_path_buf()).unwrap());
        let mock = Arc::new(MockProvider::new());
        let provider: Arc<dyn TtsProvider> = mock.clone();
        let service = mk_service(provider, cache, "Audio");

        let frontmatter = mk_frontmatter();
        let mut card = mk_card("日本語[にほんご]を");

        let prepared = service
            .prepare_from_anki_fields(&card.raw_anki_fields, &frontmatter.field_map)
            .unwrap();
        let _ = service.ensure_cached(&prepared).unwrap();

        // User edits the card after previewing.
        card.raw_anki_fields
            .insert("Front".into(), "英語[えいご]を".into());

        let prepared2 = service
            .prepare_from_anki_fields(&card.raw_anki_fields, &frontmatter.field_map)
            .unwrap();
        assert_ne!(
            prepared.filename, prepared2.filename,
            "edit should change the content-addressed filename"
        );
        let _ = service.ensure_cached(&prepared2).unwrap();
        assert_eq!(
            mock.calls.lock().unwrap().len(),
            2,
            "edit must silently trigger a fresh synth"
        );
    }
}

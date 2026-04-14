use anyhow::{Context, Result};

use crate::anki::client::AnkiClient;
use crate::anki::schema::AddNoteParams;
use crate::style::style;
use crate::template::frontmatter::Frontmatter;
use crate::tts::media::{MediaUploader, format_sound_tag};
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
    pub media: &'a dyn MediaUploader,
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
        //
        // Check against `raw_anki_fields` — the same field map
        // `prepare_from_anki_fields` reads from below — so the skip
        // decision and the synthesis decision always look at the same
        // bytes. Checking the sanitized `anki_fields` sibling would
        // drift if a future post-processor rewrote one map but not the
        // other.
        let existing = card
            .raw_anki_fields
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

        let stored = media
            .ensure_uploaded(&prepared.filename, &bytes)
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

    /// Recording `MediaUploader` for `finalize_tts` tests. Avoids the
    /// real `AnkiMediaStore` (which would hit AnkiConnect over HTTP) and
    /// lets each test inspect the exact sequence of upload calls and
    /// inject a failure after the Nth one.
    struct MockUploader {
        calls: Mutex<Vec<(String, usize)>>,
        fail_after: Option<usize>,
    }

    impl MockUploader {
        fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                fail_after: None,
            }
        }

        fn failing_after(n: usize) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                fail_after: Some(n),
            }
        }

        fn call_count(&self) -> usize {
            self.calls.lock().unwrap().len()
        }

        fn filenames(&self) -> Vec<String> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .map(|(name, _)| name.clone())
                .collect()
        }
    }

    impl MediaUploader for MockUploader {
        fn ensure_uploaded(&self, filename: &str, bytes: &[u8]) -> Result<String, TtsError> {
            let mut calls = self.calls.lock().unwrap();
            calls.push((filename.to_string(), bytes.len()));
            let n = calls.len();
            drop(calls);
            if let Some(limit) = self.fail_after
                && n > limit
            {
                return Err(TtsError::Transient(format!(
                    "mock upload failure on call {n}"
                )));
            }
            Ok(filename.to_string())
        }
    }

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

        // The skip guard must short-circuit before touching the media
        // store, so the recording mock should see zero calls.
        let uploader = MockUploader::new();
        let finalizer = TtsFinalize {
            service: &service,
            media: &uploader,
        };

        let mut cards = vec![card];
        finalize_tts(&mut cards, &frontmatter, finalizer, &|_| {}).unwrap();

        assert_eq!(
            mock.calls.lock().unwrap().len(),
            0,
            "populated target field must not trigger synthesis"
        );
        assert_eq!(
            uploader.call_count(),
            0,
            "populated target field must not trigger upload"
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
    fn finalize_happy_path_uploads_all_cards_and_rewrites_target_fields() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = Arc::new(TtsCache::new(tmp.path().to_path_buf()).unwrap());
        let mock = Arc::new(MockProvider::new());
        let provider: Arc<dyn TtsProvider> = mock.clone();
        let service = mk_service(provider, cache, "Audio");

        let frontmatter = mk_frontmatter();
        let mut cards = vec![mk_card("alpha"), mk_card("beta"), mk_card("gamma")];

        let uploader = MockUploader::new();
        let finalizer = TtsFinalize {
            service: &service,
            media: &uploader,
        };

        finalize_tts(&mut cards, &frontmatter, finalizer, &|_| {}).unwrap();

        assert_eq!(
            uploader.call_count(),
            3,
            "each card should trigger one upload"
        );
        let filenames = uploader.filenames();
        assert_eq!(
            filenames
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            3,
            "distinct card content should produce distinct filenames"
        );
        for card in &cards {
            let tag = card
                .anki_fields
                .get("Audio")
                .expect("target field must be present");
            assert!(
                tag.starts_with("[sound:") && tag.ends_with(']'),
                "target field should be rewritten to a [sound:...] tag: {tag}"
            );
            assert_eq!(
                card.raw_anki_fields.get("Audio"),
                card.anki_fields.get("Audio"),
                "raw_anki_fields must mirror the rewritten target field"
            );
        }
    }

    #[test]
    fn finalize_aborts_on_mid_run_upload_failure_with_partial_state() {
        // Pins current behavior: a failed upload mid-run returns Err
        // from `finalize_tts`. Cards already processed keep their
        // rewritten field; later cards keep whatever they had. Rollback
        // is explicitly out of scope (see the followups doc).
        let tmp = tempfile::tempdir().unwrap();
        let cache = Arc::new(TtsCache::new(tmp.path().to_path_buf()).unwrap());
        let mock = Arc::new(MockProvider::new());
        let provider: Arc<dyn TtsProvider> = mock.clone();
        let service = mk_service(provider, cache, "Audio");

        let frontmatter = mk_frontmatter();
        let mut cards = vec![mk_card("alpha"), mk_card("beta"), mk_card("gamma")];

        let uploader = MockUploader::failing_after(1);
        let finalizer = TtsFinalize {
            service: &service,
            media: &uploader,
        };

        let result = finalize_tts(&mut cards, &frontmatter, finalizer, &|_| {});
        let err = result.expect_err("upload failure must propagate");
        assert!(
            err.to_string().contains("card #2"),
            "error should identify the offending card (got: {err})"
        );
        assert_eq!(
            uploader.call_count(),
            2,
            "should attempt exactly two uploads before aborting"
        );
        assert!(
            cards[0]
                .anki_fields
                .get("Audio")
                .map(|s| s.starts_with("[sound:"))
                .unwrap_or(false),
            "first card should have been rewritten before the abort"
        );
        // The failing card (card 1 / logged as `card #2`) must not have
        // been mutated: a buggy implementation that inserted the tag
        // before calling `ensure_uploaded` would otherwise pass the
        // card-0 and card-2 assertions.
        assert_eq!(
            cards[1].anki_fields.get("Audio").map(String::as_str),
            Some(""),
            "failing card must not have its target field mutated"
        );
        assert_eq!(
            cards[1].raw_anki_fields.get("Audio").map(String::as_str),
            Some(""),
            "failing card must not have its raw target field mutated"
        );
        assert_eq!(
            cards[2].anki_fields.get("Audio").map(String::as_str),
            Some(""),
            "third card should still have its original empty target field"
        );
        assert_eq!(
            cards[2].raw_anki_fields.get("Audio").map(String::as_str),
            Some(""),
            "third card raw field should also be unchanged"
        );
    }

    #[test]
    fn finalize_deduplicates_uploads_for_identical_cards() {
        // Two cards with identical source text hit the same
        // content-addressed filename and the upload-dedup path inside
        // the store. The mock's per-call recording pins the expected
        // call count: the store layer is what deduplicates, but
        // `finalize_tts` must feed it the same filename both times for
        // dedup to kick in.
        let tmp = tempfile::tempdir().unwrap();
        let cache = Arc::new(TtsCache::new(tmp.path().to_path_buf()).unwrap());
        let mock = Arc::new(MockProvider::new());
        let provider: Arc<dyn TtsProvider> = mock.clone();
        let service = mk_service(provider, cache, "Audio");

        let frontmatter = mk_frontmatter();
        let mut cards = vec![mk_card("同じ"), mk_card("同じ")];

        let uploader = MockUploader::new();
        let finalizer = TtsFinalize {
            service: &service,
            media: &uploader,
        };

        finalize_tts(&mut cards, &frontmatter, finalizer, &|_| {}).unwrap();

        let filenames = uploader.filenames();
        assert_eq!(
            filenames.len(),
            2,
            "finalize must call the uploader once per card (dedup happens inside the store)"
        );
        assert_eq!(
            filenames[0], filenames[1],
            "identical card content must produce identical filenames"
        );
        assert_eq!(
            cards[0].anki_fields.get("Audio"),
            cards[1].anki_fields.get("Audio"),
            "both cards must end up with the same sound tag"
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

use anyhow::Result;

use crate::anki::client::AnkiClient;
use crate::template::frontmatter::Frontmatter;

use super::super::anki_import::{TtsFinalize, finalize_tts, import_cards_to_anki};
use super::super::cards::ValidatedCard;
use super::super::exporter::export_cards;
use super::types::{PipelineOutcome, PipelineProgress, PipelineStep};

/// Export the curated cards to a file. Returns the success outcome.
pub(super) fn run_export(
    final_cards: Vec<ValidatedCard>,
    output_path: &std::path::Path,
    progress: &dyn PipelineProgress,
    on_log: &(dyn Fn(&str) + Send + Sync),
) -> Result<PipelineOutcome> {
    progress.step_skip(PipelineStep::FinalizeTts);
    progress.step_start(PipelineStep::Finish, None);
    export_cards(&final_cards, output_path, on_log)?;
    progress.step_done(
        PipelineStep::Finish,
        Some(format!("exported to {}", output_path.display())),
    );
    Ok(PipelineOutcome::Success {
        message: format!(
            "Exported {} card(s) to {}",
            final_cards.len(),
            output_path.display()
        ),
        cards: final_cards,
        note_ids: Vec::new(),
        failed: false,
    })
}

/// Resolve the deck's TTS bundle (if configured) and run
/// `finalize_tts` against the curated cards. Surfaces all late-stage
/// failures — missing credentials, synth errors, upload errors — as
/// `Err(PipelineOutcome::Success { failed: true, .. })` so the caller
/// can return the curated state to the TUI instead of tearing down
/// the selection via `RunError`.
pub(super) fn run_finalize_tts_step(
    final_cards: &mut Vec<ValidatedCard>,
    tts: Option<&crate::tts::service::SessionTts>,
    frontmatter: &Frontmatter,
    progress: &dyn PipelineProgress,
    on_log: &(dyn Fn(&str) + Send + Sync),
) -> Result<(), PipelineOutcome> {
    let Some(session_tts) = tts else {
        progress.step_skip(PipelineStep::FinalizeTts);
        return Ok(());
    };
    progress.step_start(PipelineStep::FinalizeTts, None);

    let bundle = match session_tts.bundle() {
        Ok(b) => b,
        Err(e) => {
            progress.step_error(PipelineStep::FinalizeTts, &format!("{e}"));
            return Err(PipelineOutcome::Success {
                message: format!("Import failed: {e}"),
                cards: std::mem::take(final_cards),
                note_ids: Vec::new(),
                failed: true,
            });
        }
    };

    let finalizer = TtsFinalize {
        service: &bundle.service,
        media: bundle.media.as_ref(),
    };
    if let Err(e) = finalize_tts(final_cards, frontmatter, finalizer, on_log) {
        progress.step_error(PipelineStep::FinalizeTts, &format!("{e}"));
        return Err(PipelineOutcome::Success {
            message: format!("Import failed: {e}"),
            cards: std::mem::take(final_cards),
            note_ids: Vec::new(),
            failed: true,
        });
    }
    progress.step_done(PipelineStep::FinalizeTts, None);
    Ok(())
}

/// Import `final_cards` into Anki and convert the result into a
/// `PipelineOutcome`. Import errors are surfaced as
/// `PipelineOutcome::Success { failed: true, cards, .. }` so the
/// user's curated selection state survives the Done view instead of
/// getting torn down by the TUI's `RunError` handler.
pub(super) fn run_import_step(
    mut final_cards: Vec<ValidatedCard>,
    frontmatter: &Frontmatter,
    anki: &AnkiClient,
    progress: &dyn PipelineProgress,
    on_log: &(dyn Fn(&str) + Send + Sync),
) -> PipelineOutcome {
    progress.step_start(PipelineStep::Finish, None);
    let result = match import_cards_to_anki(&mut final_cards, frontmatter, anki, on_log) {
        Ok(result) => result,
        Err(e) => {
            progress.step_error(PipelineStep::Finish, &format!("{e}"));
            return PipelineOutcome::Success {
                message: format!("Import failed: {e}"),
                cards: final_cards,
                note_ids: Vec::new(),
                failed: true,
            };
        }
    };
    let note_ids = result.note_ids.clone();

    if result.failures > 0 {
        progress.step_done(
            PipelineStep::Finish,
            Some(format!(
                "{} added, {} failed",
                result.successes, result.failures
            )),
        );
        PipelineOutcome::Success {
            message: format!(
                "Import completed with errors: {} added, {} failed.",
                result.successes, result.failures
            ),
            cards: final_cards,
            note_ids,
            failed: false,
        }
    } else {
        progress.step_done(
            PipelineStep::Finish,
            Some(format!("{} card(s) added", result.successes)),
        );
        PipelineOutcome::Success {
            message: format!(
                "Successfully added {} new note(s) to \"{}\"",
                result.successes, frontmatter.deck
            ),
            cards: final_cards,
            note_ids,
            failed: false,
        }
    }
}

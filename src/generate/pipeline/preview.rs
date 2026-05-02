use super::super::cards::ValidatedCard;
use super::types::{PipelineConfig, PipelineInteraction, PipelineProgress, TtsPreviewState};

/// Synthesize preview audio for a card. On success, caches the audio
/// to disk and emits `TtsPreviewState::Ready { cache_path }` so the UI
/// can route playback through its owned audio thread. On failure, emits
/// `TtsPreviewState::Failed` with a user-facing message.
pub(super) fn handle_preview_tts(
    config: &PipelineConfig,
    interaction: &dyn PipelineInteraction,
    progress: &dyn PipelineProgress,
    card: &ValidatedCard,
) {
    let Some(session_tts) = config.tts else {
        // No TTS configured — silently drop the request. The TUI should
        // never send this command in that case, but defend anyway.
        return;
    };
    let card_id = card.card_id;

    interaction.tts_state(card_id, TtsPreviewState::Synthesizing);

    // First preview in a session materializes the bundle via
    // `spec::resolve`. If credentials are missing the failure surfaces
    // here as a per-card `Failed` state, not a session-wide fatal — the
    // user can fix the env and retry without losing their curation.
    let bundle = match session_tts.bundle() {
        Ok(b) => b,
        Err(e) => {
            progress.log(&format!("TTS unavailable: {e:#}"));
            interaction.tts_state(card_id, TtsPreviewState::Failed(format!("{e:#}")));
            return;
        }
    };

    let prepared = match bundle
        .service
        .prepare_from_anki_fields(&card.raw_anki_fields, &config.frontmatter.field_map)
    {
        Ok(p) => p,
        Err(e) => {
            progress.log(&format!("TTS prepare failed: {e:#}"));
            interaction.tts_state(card_id, TtsPreviewState::Failed(format!("{e:#}")));
            return;
        }
    };

    match bundle.service.ensure_cached(&prepared) {
        Ok(_) => {
            progress.log(&format!(
                "TTS ready: {} chars → {}",
                prepared.spoken_chars, prepared.filename
            ));
            interaction.tts_state(
                card_id,
                TtsPreviewState::Ready {
                    cache_path: prepared.cache_path,
                },
            );
        }
        Err(e) => {
            progress.log(&format!("TTS synthesis failed: {e:#}"));
            interaction.tts_state(card_id, TtsPreviewState::Failed(format!("{e:#}")));
        }
    }
}

mod backend;
mod editor;
mod effects;
pub(crate) mod events;
mod history;
mod keys;
mod keys_input;
mod keys_selection;
mod prompt_picker;
mod render;
mod render_chrome;
mod runner;
mod screens;
mod state;
mod widgets;

pub use events::{BackendEvent, SessionInfo, StepStatus, TtsUiState, WorkerCommand};
pub use runner::run_tui;

// Re-export PipelineStep from the shared pipeline module
pub use super::pipeline::PipelineStep;

#[cfg(test)]
mod tests {
    use super::editor::parse_edited_anki_fields;
    use super::events::{BackendEvent, TtsUiState, WorkerCommand};
    use super::runner::any_card_synthesizing;
    use super::screens::selection::SelectionState;
    use super::state::{App as AppState, AppMode, MAX_THINKING_CHARS};
    use crate::generate::cards::{ValidatedCard, next_card_id};
    use crate::tui::theme::Glyphs;
    use indexmap::IndexMap;
    use std::sync::mpsc;

    fn mk_app() -> AppState {
        let (_tx_events, rx_events) = mpsc::channel();
        let (tx_cmd, _rx_cmd) = mpsc::sync_channel::<WorkerCommand>(10);
        let mut app = AppState::new(None, Glyphs::from_config(), rx_events, tx_cmd);
        app.mode = AppMode::Running;
        app
    }

    fn mk_card() -> ValidatedCard {
        use std::collections::HashMap;
        let mut fields: HashMap<String, String> = HashMap::new();
        fields.insert("front".into(), "x".into());
        let mut anki_fields: IndexMap<String, String> = IndexMap::new();
        anki_fields.insert("Front".into(), "x".into());
        ValidatedCard {
            card_id: next_card_id(),
            fields,
            anki_fields: anki_fields.clone(),
            raw_anki_fields: anki_fields,
            is_duplicate: false,
            duplicate_note_id: None,
            duplicate_fields: None,
            flags: Vec::new(),
            model: "test".into(),
        }
    }

    /// Guards Enter/Esc in `handle_key_selection` against a race with
    /// an in-flight TTS preview. The guard must be selection-global,
    /// not focused-row-local: the worker command channel is a shared
    /// FIFO, so a `Selection` / `Cancel` sent while *any* card's
    /// preview is in flight still queues behind that `PreviewTts` and
    /// re-opens the race. The actual key handler sits on an `App`
    /// value that requires mpsc plumbing to construct, so we drive
    /// the pure helper directly.
    #[test]
    fn any_synthesizing_guard_fires_even_when_focus_moves() {
        let a = mk_card();
        let b = mk_card();
        let a_id = a.card_id;
        let b_id = b.card_id;
        let mut state = SelectionState::new(vec![a, b]);

        // Idle: guard off.
        assert!(!any_card_synthesizing(&state));

        // Card A synthesizing while focused on A: guard on.
        state.tts_states.insert(a_id, TtsUiState::Synthesizing);
        assert!(any_card_synthesizing(&state));

        // Move focus to B (which is NOT synthesizing) — guard must
        // stay on. This is the regression a focused-row-local check
        // would miss: pressing `p` on A, arrowing down, then hitting
        // Enter would queue a Selection behind A's in-flight
        // PreviewTts and trigger the race from issue #9.
        state.move_down();
        assert_eq!(state.cursor, 1);
        assert!(
            any_card_synthesizing(&state),
            "guard must stay on while any card is synthesizing, even after cursor moves"
        );

        // B in a terminal Ready state; A still synthesizing: guard on.
        state.tts_states.insert(
            b_id,
            TtsUiState::Ready {
                cache_path: std::path::PathBuf::from("/tmp/x.mp3"),
            },
        );
        assert!(any_card_synthesizing(&state));

        // A resolves to Ready — no more in-flight previews: guard off.
        state.tts_states.insert(
            a_id,
            TtsUiState::Ready {
                cache_path: std::path::PathBuf::from("/tmp/a.mp3"),
            },
        );
        assert!(!any_card_synthesizing(&state));
    }

    #[test]
    fn thinking_delta_is_ephemeral() {
        let mut app = mk_app();
        app.handle_backend_event(BackendEvent::Log("normal".into()));
        app.handle_backend_event(BackendEvent::ThinkingDelta("raw thought".into()));

        assert_eq!(app.thinking, "raw thought");
        assert_eq!(app.logs, vec!["normal"]);
        assert!(app.steps.iter().all(|step| step.logs != ["raw thought"]));
    }

    #[test]
    fn thinking_persists_on_reset_but_clears_on_done_error_and_cancel_discard() {
        let mut app = mk_app();
        app.handle_backend_event(BackendEvent::ThinkingDelta("first".into()));
        app.handle_backend_event(BackendEvent::ThinkingReset);
        assert_eq!(app.thinking, "first");

        app.handle_backend_event(BackendEvent::ThinkingClear);
        assert!(app.thinking.is_empty());

        app.handle_backend_event(BackendEvent::ThinkingDelta("second".into()));
        app.handle_backend_event(BackendEvent::RunError("failed".into()));
        assert!(app.thinking.is_empty());

        let mut app = mk_app();
        app.handle_backend_event(BackendEvent::ThinkingDelta("third".into()));
        app.pending_cancels = 1;
        app.handle_backend_event(BackendEvent::ThinkingDelta(" stale".into()));
        app.handle_backend_event(BackendEvent::RunDone {
            message: String::new(),
            cards: Vec::new(),
            note_ids: Vec::new(),
            failed: false,
        });
        assert_eq!(app.thinking, "third");
    }

    #[test]
    fn thinking_buffer_is_bounded() {
        let mut app = mk_app();
        app.handle_backend_event(BackendEvent::ThinkingDelta(
            "x".repeat(MAX_THINKING_CHARS + 10),
        ));
        assert_eq!(app.thinking.len(), MAX_THINKING_CHARS);
    }

    #[test]
    fn edited_yaml_first_field_lookup_is_order_independent() {
        // User rearranged fields in `$EDITOR` so the note type's first
        // field (`Front`) is no longer the first key in the YAML.
        let yaml = "Back: gloss\nAudio: ''\nFront: 日本語\n";
        let parsed = parse_edited_anki_fields(yaml).unwrap();

        // The authoritative first-field name comes from
        // `SessionInfo.first_field_name` — sourced from
        // `validation.note_type_fields[0]`, not YAML insertion order.
        let first_field_name = "Front";
        let first_field_value = parsed.get(first_field_name).cloned().unwrap_or_default();

        assert_eq!(
            first_field_value, "日本語",
            "lookup by authoritative first-field name must survive YAML reorder"
        );

        // Guard against the pre-fix regression: the first *entry* of the
        // parsed map is `Back`, not `Front` — if we ever went back to
        // `values().next()` this test would catch it.
        let naive_first = parsed.values().next().cloned().unwrap_or_default();
        assert_ne!(
            naive_first, first_field_value,
            "naive insertion-order lookup must not match authoritative lookup \
             in the reordered case"
        );
    }
}

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
    use super::effects::Effect;
    use super::events::{BackendEvent, SessionInfo, TtsUiState, WorkerCommand};
    use super::runner::{
        any_card_synthesizing, apply_delete_from_anki_result, apply_preview_tts_send_failure,
        send_initial_start,
    };
    use super::screens::review::ReviewState;
    use super::screens::selection::SelectionState;
    use super::state::{App as AppState, AppMode, AudioStatus, MAX_THINKING_CHARS};
    use crate::generate::cards::{ValidatedCard, next_card_id};
    use crate::generate::process::FlaggedCard;
    use crate::tui::line_input::LineInput;
    use crate::tui::theme::Glyphs;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use indexmap::IndexMap;
    use serial_test::serial;
    use std::path::PathBuf;
    use std::sync::mpsc;

    fn mk_app() -> AppState {
        let mut app = AppState::new(
            None,
            Glyphs::from_config(),
            Default::default(),
            AudioStatus::Unavailable,
        );
        app.mode = AppMode::Running;
        app
    }

    fn mk_card() -> ValidatedCard {
        mk_card_with_front("x")
    }

    fn mk_card_with_front(front: &str) -> ValidatedCard {
        use std::collections::HashMap;
        let mut fields: HashMap<String, String> = HashMap::new();
        fields.insert("front".into(), front.into());
        let mut anki_fields: IndexMap<String, String> = IndexMap::new();
        anki_fields.insert("Front".into(), front.into());
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

    fn mk_session_info(tts_configured: bool) -> SessionInfo {
        SessionInfo {
            deck: "deck".into(),
            note_type: "note".into(),
            model: "model".into(),
            available_models: Vec::new(),
            field_map: IndexMap::new(),
            first_field_name: "Front".into(),
            tts_configured,
            post_select_configured: false,
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn flagged_card(card: ValidatedCard) -> FlaggedCard {
        FlaggedCard {
            card,
            reason: "review".into(),
        }
    }

    fn selecting_state(app: &AppState) -> &SelectionState {
        let AppMode::Selecting(state) = &app.mode else {
            panic!("expected selection mode");
        };
        state
    }

    struct HomeGuard {
        original: Option<std::ffi::OsString>,
    }

    impl HomeGuard {
        fn set(path: &std::path::Path) -> Self {
            let original = std::env::var_os("HOME");
            unsafe { std::env::set_var("HOME", path) };
            Self { original }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            if let Some(value) = self.original.take() {
                unsafe { std::env::set_var("HOME", value) };
            } else {
                unsafe { std::env::remove_var("HOME") };
            }
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
        let _ = app.handle_backend_event(BackendEvent::Log("normal".into()));
        let _ = app.handle_backend_event(BackendEvent::ThinkingDelta("raw thought".into()));

        assert_eq!(app.thinking, "raw thought");
        assert_eq!(app.logs, vec!["normal"]);
        assert!(app.steps.iter().all(|step| step.logs != ["raw thought"]));
    }

    #[test]
    fn thinking_persists_on_reset_but_clears_on_done_error_and_cancel_discard() {
        let mut app = mk_app();
        let _ = app.handle_backend_event(BackendEvent::ThinkingDelta("first".into()));
        let _ = app.handle_backend_event(BackendEvent::ThinkingReset);
        assert_eq!(app.thinking, "first");

        let _ = app.handle_backend_event(BackendEvent::ThinkingClear);
        assert!(app.thinking.is_empty());

        let _ = app.handle_backend_event(BackendEvent::ThinkingDelta("second".into()));
        let _ = app.handle_backend_event(BackendEvent::RunError("failed".into()));
        assert!(app.thinking.is_empty());

        let mut app = mk_app();
        let _ = app.handle_backend_event(BackendEvent::ThinkingDelta("third".into()));
        app.pending_cancels = 1;
        let effects = app.handle_backend_event(BackendEvent::ThinkingDelta(" stale".into()));
        assert!(effects.is_empty());
        let _ = app.handle_backend_event(BackendEvent::RunDone {
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
        let _ = app.handle_backend_event(BackendEvent::ThinkingDelta(
            "x".repeat(MAX_THINKING_CHARS + 10),
        ));
        assert_eq!(app.thinking.len(), MAX_THINKING_CHARS);
    }

    #[test]
    fn app_new_initial_term_is_pure_state_setup() {
        let app = AppState::new(
            Some("term".into()),
            Glyphs::from_config(),
            Default::default(),
            AudioStatus::Unavailable,
        );

        assert!(matches!(app.mode, AppMode::Running));
        assert_eq!(app.last_term.as_deref(), Some("term"));
    }

    #[test]
    fn send_initial_start_emits_start_for_initial_term_only() {
        let (tx, rx) = mpsc::sync_channel(1);

        send_initial_start(&tx, Some("term".into()));

        assert!(matches!(
            rx.try_recv(),
            Ok(WorkerCommand::Start {
                term,
                enable_thinking_stream: true,
            }) if term == "term"
        ));
        assert!(rx.try_recv().is_err());

        send_initial_start(&tx, None);

        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn session_ready_returns_audio_player_start_effect() {
        let mut app = mk_app();
        app.audio_status = AudioStatus::Available;

        let effects = app.handle_backend_event(BackendEvent::SessionReady(mk_session_info(true)));

        assert!(app.session_info.is_some());
        assert!(matches!(effects.as_slice(), [Effect::StartAudioPlayer]));
    }

    #[test]
    fn request_selection_batch_continuation_returns_worker_effect() {
        let mut app = mk_app();
        let first = mk_card_with_front("first");
        app.batch_queue = vec!["next".into()];
        app.batch_progress = Some((1, 2));

        let effects = app.handle_backend_event(BackendEvent::RequestSelection(vec![first]));

        assert_eq!(app.batch_queue, Vec::<String>::new());
        assert_eq!(app.batch_progress, Some((2, 2)));
        assert_eq!(app.last_term.as_deref(), Some("next"));
        assert_eq!(app.batch_cards.len(), 1);
        assert!(matches!(
            effects.as_slice(),
            [Effect::SendWorker(WorkerCommand::RefreshWithTerm(term))] if term == "next"
        ));
    }

    #[test]
    fn append_cards_batch_continuation_returns_worker_effect() {
        let mut app = mk_app();
        let first = mk_card_with_front("first");
        app.batch_cards.push(first);
        app.batch_queue = vec!["next".into()];
        app.batch_progress = Some((1, 2));

        let effects =
            app.handle_backend_event(BackendEvent::AppendCards(vec![mk_card_with_front(
                "second",
            )]));

        assert_eq!(app.batch_queue, Vec::<String>::new());
        assert_eq!(app.batch_progress, Some((2, 2)));
        assert_eq!(app.last_term.as_deref(), Some("next"));
        assert_eq!(app.batch_cards.len(), 2);
        assert!(matches!(
            effects.as_slice(),
            [Effect::SendWorker(WorkerCommand::RefreshWithTerm(term))] if term == "next"
        ));
    }

    #[test]
    fn batch_run_error_returns_start_effect_for_next_term() {
        let mut app = mk_app();
        app.last_term = Some("failed".into());
        app.batch_queue = vec!["next".into()];
        app.batch_progress = Some((1, 2));

        let effects = app.handle_backend_event(BackendEvent::RunError("boom".into()));

        assert_eq!(app.batch_queue, Vec::<String>::new());
        assert_eq!(app.batch_progress, Some((2, 2)));
        assert_eq!(app.last_term.as_deref(), Some("next"));
        assert!(matches!(app.mode, AppMode::Running));
        assert!(matches!(
            effects.as_slice(),
            [Effect::SendWorker(WorkerCommand::Start { term, enable_thinking_stream: false })]
                if term == "next"
        ));
    }

    #[test]
    fn tts_ready_returns_play_audio_effect_for_existing_card() {
        let mut app = mk_app();
        let card = mk_card();
        let card_id = card.card_id;
        app.mode = AppMode::Selecting(SelectionState::new(vec![card]));
        let path = std::path::PathBuf::from("/tmp/audio.mp3");

        let effects = app.handle_backend_event(BackendEvent::TtsState {
            card_id,
            state: TtsUiState::Ready {
                cache_path: path.clone(),
            },
        });

        assert!(matches!(
            effects.as_slice(),
            [Effect::PlayAudio { card_id: id, path: effect_path }]
                if *id == card_id && effect_path == &path
        ));
        let AppMode::Selecting(state) = &app.mode else {
            panic!("expected selection mode");
        };
        assert!(matches!(
            state.tts_states.get(&card_id),
            Some(TtsUiState::Ready { cache_path }) if cache_path == &path
        ));
    }

    #[test]
    fn stale_tts_state_returns_no_effect_and_stores_no_state() {
        let mut app = mk_app();
        let card = mk_card();
        let stale_ready_id = next_card_id();
        let stale_synthesizing_id = next_card_id();
        app.mode = AppMode::Selecting(SelectionState::new(vec![card]));

        let effects = app.handle_backend_event(BackendEvent::TtsState {
            card_id: stale_ready_id,
            state: TtsUiState::Ready {
                cache_path: std::path::PathBuf::from("/tmp/stale.mp3"),
            },
        });

        assert!(effects.is_empty());
        assert!(
            !selecting_state(&app)
                .tts_states
                .contains_key(&stale_ready_id)
        );

        let effects = app.handle_backend_event(BackendEvent::TtsState {
            card_id: stale_synthesizing_id,
            state: TtsUiState::Synthesizing,
        });

        assert!(effects.is_empty());
        assert!(
            !selecting_state(&app)
                .tts_states
                .contains_key(&stale_synthesizing_id)
        );
    }

    #[test]
    fn selection_model_picker_defers_model_change() {
        let mut app = mk_app();
        let mut info = mk_session_info(false);
        info.model = "old".into();
        info.available_models = vec!["old".into(), "new".into()];
        app.session_info = Some(info);
        app.mode = AppMode::Selecting(SelectionState::new(vec![mk_card()]));

        app.open_model_picker();
        let effects = app.handle_key(key(KeyCode::Down));
        assert!(effects.is_empty());
        let effects = app.handle_key(key(KeyCode::Enter));

        assert!(effects.is_empty());
        assert_eq!(app.pending_model.as_deref(), Some("new"));
        assert_eq!(app.session_info.as_ref().unwrap().model, "new");
        assert!(app.model_picker.is_none());
        assert!(matches!(&app.toast, Some(toast) if toast.message == "Model: new"));
    }

    #[test]
    fn selection_refresh_applies_deferred_model_as_effects() {
        let mut app = mk_app();
        app.last_term = Some("term".into());
        app.pending_model = Some("new".into());
        app.mode = AppMode::Selecting(SelectionState::new(vec![mk_card()]));

        let effects = app.handle_key(key(KeyCode::Char('r')));

        assert_eq!(app.pending_model, None);
        assert_eq!(app.pending_cancels, 1);
        assert!(selecting_state(&app).refresh_in_flight);
        assert!(matches!(
            effects.as_slice(),
            [
                Effect::SendWorker(WorkerCommand::Cancel),
                Effect::SendWorker(WorkerCommand::SetModel(model)),
                Effect::SendWorker(WorkerCommand::Start { term, enable_thinking_stream: true }),
            ] if model == "new" && term == "term"
        ));
    }

    #[test]
    fn preview_queue_full_rollback_clears_optimistic_tts_state() {
        let mut app = mk_app();
        app.audio_status = AudioStatus::Ready;
        app.session_info = Some(mk_session_info(true));
        let card = mk_card();
        let card_id = card.card_id;
        app.mode = AppMode::Selecting(SelectionState::new(vec![card]));

        let effects = app.handle_key(key(KeyCode::Char('p')));

        assert!(matches!(
            effects.as_slice(),
            [Effect::TryPreviewTts { card_id: id, card: effect_card }]
                if *id == card_id && effect_card.card_id == card_id
        ));
        assert!(matches!(
            selecting_state(&app).tts_states.get(&card_id),
            Some(TtsUiState::Synthesizing)
        ));

        apply_preview_tts_send_failure(&mut app, card_id, false);

        assert!(!selecting_state(&app).tts_states.contains_key(&card_id));
        assert!(
            matches!(&app.toast, Some(toast) if toast.message == "Preview queue full — try again")
        );
    }

    #[test]
    fn pending_cancel_discards_backend_events_until_completion() {
        let mut app = mk_app();
        app.mode = AppMode::Input(LineInput::new("fresh".into()));
        app.pending_cancels = 2;

        let effects = app.handle_backend_event(BackendEvent::Log("stale".into()));
        assert!(effects.is_empty());
        assert!(app.logs.is_empty());
        assert_eq!(app.pending_cancels, 2);

        let effects = app.handle_backend_event(BackendEvent::RunDone {
            message: "stale done".into(),
            cards: Vec::new(),
            note_ids: Vec::new(),
            failed: false,
        });
        assert!(effects.is_empty());
        assert_eq!(app.pending_cancels, 1);
        assert!(matches!(app.mode, AppMode::Input(_)));

        let effects = app.handle_backend_event(BackendEvent::RunError("stale error".into()));
        assert!(effects.is_empty());
        assert_eq!(app.pending_cancels, 0);
        assert!(matches!(app.mode, AppMode::Input(_)));
    }

    #[test]
    fn selection_confirm_and_cancel_block_while_any_tts_synthesizes() {
        fn app_with_synthesizing_unfocused_card() -> (AppState, u64, u64) {
            let mut app = mk_app();
            app.pending_model = Some("new".into());
            app.batch_queue = vec!["queued".into()];
            app.batch_progress = Some((1, 2));
            app.batch_cards.push(mk_card_with_front("batched"));
            let a = mk_card_with_front("a");
            let b = mk_card_with_front("b");
            let a_id = a.card_id;
            let b_id = b.card_id;
            let mut selection = SelectionState::new(vec![a, b]);
            selection.selected.insert(b_id);
            selection.tts_states.insert(a_id, TtsUiState::Synthesizing);
            selection.move_down();
            app.mode = AppMode::Selecting(selection);
            (app, a_id, b_id)
        }

        let (mut app, synthesizing_id, selected_id) = app_with_synthesizing_unfocused_card();
        let effects = app.handle_key(key(KeyCode::Enter));
        assert!(effects.is_empty());
        assert_eq!(app.pending_model.as_deref(), Some("new"));
        assert_eq!(app.batch_queue, vec!["queued"]);
        assert_eq!(app.batch_progress, Some((1, 2)));
        assert_eq!(app.batch_cards.len(), 1);
        let state = selecting_state(&app);
        assert!(state.selected.contains(&selected_id));
        assert!(matches!(
            state.tts_states.get(&synthesizing_id),
            Some(TtsUiState::Synthesizing)
        ));
        assert!(matches!(&app.toast, Some(toast) if toast.message == "TTS preview in progress"));

        let (mut app, synthesizing_id, selected_id) = app_with_synthesizing_unfocused_card();
        let effects = app.handle_key(key(KeyCode::Esc));
        assert!(effects.is_empty());
        assert_eq!(app.pending_cancels, 0);
        assert_eq!(app.pending_model.as_deref(), Some("new"));
        assert_eq!(app.batch_queue, vec!["queued"]);
        assert_eq!(app.batch_progress, Some((1, 2)));
        assert_eq!(app.batch_cards.len(), 1);
        let state = selecting_state(&app);
        assert!(state.selected.contains(&selected_id));
        assert!(matches!(
            state.tts_states.get(&synthesizing_id),
            Some(TtsUiState::Synthesizing)
        ));
        assert!(matches!(&app.toast, Some(toast) if toast.message == "TTS preview in progress"));
    }

    #[test]
    fn input_enter_returns_start_effect_for_single_term() {
        let mut app = mk_app();
        app.mode = AppMode::Input(LineInput::new("term".into()));

        let effects = app.handle_key(key(KeyCode::Enter));

        assert!(matches!(app.mode, AppMode::Running));
        assert_eq!(app.last_term.as_deref(), Some("term"));
        assert!(matches!(
            effects.as_slice(),
            [Effect::SendWorker(WorkerCommand::Start { term, enable_thinking_stream: true })]
                if term == "term"
        ));
    }

    #[test]
    fn running_escape_returns_cancel_effect_after_state_reset() {
        let mut app = mk_app();
        app.batch_queue = vec!["queued".into()];
        app.batch_progress = Some((1, 2));
        app.batch_cards.push(mk_card());

        let effects = app.handle_key(key(KeyCode::Esc));

        assert!(matches!(app.mode, AppMode::Input(_)));
        assert!(app.batch_queue.is_empty());
        assert_eq!(app.batch_progress, None);
        assert!(app.batch_cards.is_empty());
        assert_eq!(app.pending_cancels, 1);
        assert!(matches!(
            effects.as_slice(),
            [Effect::SendWorker(WorkerCommand::Cancel)]
        ));
    }

    #[test]
    fn review_completion_returns_review_effect() {
        let mut app = mk_app();
        let card = mk_card();
        app.mode = AppMode::Reviewing(ReviewState::new(vec![flagged_card(card)]));

        let effects = app.handle_key(key(KeyCode::Enter));

        assert!(matches!(app.mode, AppMode::Running));
        assert!(matches!(
            effects.as_slice(),
            [Effect::SendWorker(WorkerCommand::Review(decisions))] if decisions == &vec![true]
        ));
    }

    #[test]
    fn done_copy_returns_copy_cards_effect() {
        let mut app = mk_app();
        let card = mk_card();
        app.mode = AppMode::Done {
            message: "done".into(),
            cards: vec![card.clone()],
            note_ids: vec![1],
            failed: false,
        };

        let effects = app.handle_key(key(KeyCode::Char('c')));

        assert!(matches!(
            effects.as_slice(),
            [Effect::CopyCards(cards)] if cards.len() == 1 && cards[0].card_id == card.card_id
        ));
    }

    #[test]
    #[serial]
    fn done_play_returns_play_audio_effect_for_cached_audio() {
        let tmp_home = tempfile::tempdir().unwrap();
        let _home = HomeGuard::set(tmp_home.path());
        let mut app = mk_app();
        let mut card = mk_card();
        let filename = format!("anki-llm-test-{}.mp3", card.card_id);
        card.raw_anki_fields
            .insert("Audio".into(), format!("[sound:{filename}]"));
        let cache_dir = crate::tts::cache::TtsCache::default_dir().unwrap();
        std::fs::create_dir_all(&cache_dir).unwrap();
        let path = cache_dir.join(&filename);
        std::fs::write(&path, b"audio").unwrap();
        app.session_info = Some(mk_session_info(true));
        app.audio_status = AudioStatus::Ready;
        app.mode = AppMode::Done {
            message: "done".into(),
            cards: vec![card.clone()],
            note_ids: Vec::new(),
            failed: false,
        };

        let effects = app.handle_key(key(KeyCode::Char('p')));

        assert!(matches!(
            effects.as_slice(),
            [Effect::PlayAudio { card_id, path: effect_path }]
                if *card_id == card.card_id && effect_path == &PathBuf::from(&path)
        ));
        std::fs::remove_file(path).ok();
    }

    #[test]
    #[serial]
    fn done_play_without_cached_audio_sets_toast_and_returns_no_effect() {
        let tmp_home = tempfile::tempdir().unwrap();
        let _home = HomeGuard::set(tmp_home.path());
        let mut app = mk_app();
        let mut card = mk_card();
        card.raw_anki_fields.insert(
            "Audio".into(),
            format!("[sound:anki-llm-missing-{}.mp3]", card.card_id),
        );
        app.session_info = Some(mk_session_info(true));
        app.audio_status = AudioStatus::Ready;
        app.mode = AppMode::Done {
            message: "done".into(),
            cards: vec![card],
            note_ids: Vec::new(),
            failed: false,
        };

        let effects = app.handle_key(key(KeyCode::Char('p')));

        assert!(effects.is_empty());
        assert!(matches!(&app.toast, Some(toast) if toast.message == "No cached audio to play"));
    }

    #[test]
    fn done_delete_returns_delete_effect_without_mutating_cards() {
        let mut app = mk_app();
        let card = mk_card();
        app.mode = AppMode::Done {
            message: "done".into(),
            cards: vec![card.clone()],
            note_ids: vec![10, 11],
            failed: false,
        };

        let effects = app.handle_key(key(KeyCode::Char('d')));

        assert!(matches!(
            effects.as_slice(),
            [Effect::DeleteFromAnki { note_ids }] if note_ids == &vec![10, 11]
        ));
        let AppMode::Done {
            cards, note_ids, ..
        } = &app.mode
        else {
            panic!("expected done mode");
        };
        assert_eq!(cards.len(), 1);
        assert_eq!(note_ids, &vec![10, 11]);
    }

    #[test]
    fn delete_result_success_clears_done_cards_and_notes() {
        let mut app = mk_app();
        app.mode = AppMode::Done {
            message: "done".into(),
            cards: vec![mk_card()],
            note_ids: vec![10, 11],
            failed: false,
        };

        apply_delete_from_anki_result::<&str>(&mut app, 2, Ok(()));

        let AppMode::Done {
            message,
            cards,
            note_ids,
            ..
        } = &app.mode
        else {
            panic!("expected done mode");
        };
        assert!(cards.is_empty());
        assert!(note_ids.is_empty());
        assert_eq!(message, "Deleted 2 note(s) from Anki.");
        assert!(matches!(&app.toast, Some(toast) if toast.message == "Deleted 2 note(s)"));
    }

    #[test]
    fn delete_result_failure_preserves_done_cards_and_notes() {
        let mut app = mk_app();
        app.mode = AppMode::Done {
            message: "done".into(),
            cards: vec![mk_card()],
            note_ids: vec![10, 11],
            failed: false,
        };

        apply_delete_from_anki_result(&mut app, 2, Err("boom"));

        let AppMode::Done {
            message,
            cards,
            note_ids,
            ..
        } = &app.mode
        else {
            panic!("expected done mode");
        };
        assert_eq!(cards.len(), 1);
        assert_eq!(note_ids, &vec![10, 11]);
        assert_eq!(message, "done");
        assert!(matches!(&app.toast, Some(toast) if toast.message == "Delete failed: boom"));
    }

    #[test]
    fn done_quit_preserves_natural_exit_semantics() {
        let mut app = mk_app();
        app.mode = AppMode::Done {
            message: "done".into(),
            cards: Vec::new(),
            note_ids: Vec::new(),
            failed: false,
        };

        let effects = app.handle_key(key(KeyCode::Char('q')));

        assert!(app.should_quit);
        assert!(!app.user_quit);
        assert!(matches!(
            effects.as_slice(),
            [Effect::SendWorker(WorkerCommand::Quit)]
        ));
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

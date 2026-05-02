use crossterm::event::{Event, KeyCode, KeyModifiers};

use super::effects::Effect;
use super::events::{TtsUiState, WorkerCommand};
use super::runner::any_card_synthesizing;
use super::state::{App, AppMode, Toast};

use crate::tui::line_input::LineInput;

impl App {
    pub(super) fn handle_key_selection(&mut self, key: crossterm::event::KeyEvent) -> Vec<Effect> {
        let mut effects = Vec::new();
        let audio_ready = self.audio_ready();
        let AppMode::Selecting(ref mut state) = self.mode else {
            return effects;
        };

        // When the inline feedback input is active (regen), route keys there
        if state.feedback_input.is_some() {
            match key.code {
                KeyCode::Enter => {
                    let feedback = state
                        .feedback_input
                        .as_ref()
                        .map(|i| i.value().trim().to_string())
                        .unwrap_or_default();
                    state.feedback_input = None;
                    if let Some(card) = state.cards.get(state.cursor).cloned()
                        && !feedback.is_empty()
                        && state.regen_in_flight.is_none()
                    {
                        state.regen_in_flight = Some(card.card_id);
                        effects.push(Effect::SendWorker(WorkerCommand::RegenerateCard {
                            card,
                            feedback,
                        }));
                    }
                }
                KeyCode::Esc => {
                    state.feedback_input = None;
                }
                _ => {
                    if let Some(ref mut input) = state.feedback_input {
                        input.handle_event(&Event::Key(key));
                    }
                }
            }
            return effects;
        }

        // When the inline term input is active, route keys there
        if state.term_input.is_some() {
            match key.code {
                KeyCode::Enter => {
                    let term = state
                        .term_input
                        .as_ref()
                        .map(|i| i.value().trim().to_string())
                        .unwrap_or_default();
                    state.term_input = None;
                    if !term.is_empty() && !state.refresh_in_flight {
                        self.history.push(&term);
                        self.last_term = Some(term.clone());
                        if let Some(model) = self.pending_model.take() {
                            // Deferred model change: cancel, switch, start fresh.
                            state.refresh_in_flight = true;
                            effects.push(Effect::SendWorker(WorkerCommand::Cancel));
                            self.pending_cancels += 1;
                            effects.push(Effect::SendWorker(WorkerCommand::SetModel(model)));
                            effects.push(Effect::SendWorker(WorkerCommand::Start {
                                term,
                                enable_thinking_stream: true,
                            }));
                        } else {
                            state.refresh_in_flight = true;
                            effects.push(Effect::SendWorker(WorkerCommand::RefreshWithTerm(term)));
                        }
                    }
                }
                KeyCode::Esc => {
                    state.term_input = None;
                }
                _ => {
                    if let Some(ref mut input) = state.term_input {
                        input.handle_event(&Event::Key(key));
                    }
                }
            }
            return effects;
        }

        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                effects.push(Effect::Quit);
            }
            KeyCode::Up | KeyCode::Char('k') => state.move_up(),
            KeyCode::Down | KeyCode::Char('j') => state.move_down(),
            KeyCode::Char(' ') => state.toggle_current(),
            KeyCode::Char('a') => state.select_all(),
            KeyCode::Char('n') => state.select_none(),
            KeyCode::Char('r') if !state.refresh_in_flight => {
                if let Some(model) = self.pending_model.take() {
                    // Deferred model change: cancel current pipeline, switch
                    // model, and start a fresh one. Stay in selection view —
                    // new cards will be appended when they arrive.
                    state.refresh_in_flight = true;
                    effects.push(Effect::SendWorker(WorkerCommand::Cancel));
                    self.pending_cancels += 1;
                    effects.push(Effect::SendWorker(WorkerCommand::SetModel(model)));
                    let term = self.last_term.clone().unwrap_or_default();
                    effects.push(Effect::SendWorker(WorkerCommand::Start {
                        term,
                        enable_thinking_stream: true,
                    }));
                } else {
                    state.refresh_in_flight = true;
                    effects.push(Effect::SendWorker(WorkerCommand::Refresh));
                }
            }
            KeyCode::Char('t') if !state.refresh_in_flight => {
                state.term_input = Some(LineInput::default());
            }
            KeyCode::Char('e') => {
                effects.push(Effect::OpenEditor {
                    card_index: state.cursor,
                });
            }
            KeyCode::Char('R') if state.regen_in_flight.is_none() => {
                // Don't allow regenerating duplicates
                let is_dup = state
                    .cards
                    .get(state.cursor)
                    .map(|c| c.is_duplicate)
                    .unwrap_or(true);
                if !is_dup {
                    state.feedback_input = Some(LineInput::default());
                }
            }
            KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.open_model_picker();
            }
            KeyCode::Esc => {
                // Block while the focused card has a TTS preview in
                // flight: the worker's action channel is FIFO, so a
                // `Cancel` queued behind `PreviewTts` still runs after
                // synthesis bills the user and may race with import.
                // The preview resolves in a couple seconds; asking the
                // user to wait is simpler than the worker-side
                // preemption refactor.
                if any_card_synthesizing(state) {
                    self.toast = Some(Toast {
                        message: "TTS preview in progress".into(),
                        tick: self.tick,
                    });
                    return effects;
                }
                self.pending_model = None;
                self.batch_queue.clear();
                self.batch_progress = None;
                self.batch_cards.clear();
                effects.push(Effect::SendWorker(WorkerCommand::Cancel));
                self.pending_cancels += 1;
                self.reset_for_new_run();
                self.mode = AppMode::Input(LineInput::default());
            }
            KeyCode::Char('q') => {
                effects.push(Effect::Quit);
            }
            KeyCode::Enter if !state.refresh_in_flight => {
                // See the Esc arm for the race rationale; same guard.
                if any_card_synthesizing(state) {
                    self.toast = Some(Toast {
                        message: "TTS preview in progress".into(),
                        tick: self.tick,
                    });
                    return effects;
                }
                self.pending_model = None;
                let AppMode::Selecting(state) = std::mem::replace(&mut self.mode, AppMode::Running)
                else {
                    return effects;
                };
                let cards = state.selected_cards_in_order();
                let skip_post_select = state.skip_post_select;
                effects.push(Effect::SendWorker(WorkerCommand::Selection {
                    cards,
                    skip_post_select,
                }));
            }
            KeyCode::Char('f') => {
                state.force_toggle_duplicate();
            }
            KeyCode::Char('z') => {
                let has_post_select = self
                    .session_info
                    .as_ref()
                    .map(|info| info.post_select_configured)
                    .unwrap_or(false);
                if has_post_select {
                    state.toggle_skip_post_select();
                }
            }
            KeyCode::Char('d') => {
                state.remove_current();
            }
            KeyCode::Char('c') => {
                if let Some(card) = state.cards.get(state.cursor).cloned() {
                    effects.push(Effect::CopyCards(vec![card]));
                }
            }
            KeyCode::Char('p') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                // TTS preview: hidden keybind unless the session actually
                // supports it (prompt has `tts:` AND runner has started audio).
                let enabled = self
                    .session_info
                    .as_ref()
                    .map(|info| info.tts_configured)
                    .unwrap_or(false);
                if !enabled || !audio_ready {
                    return effects;
                }
                let Some(card) = state.cards.get(state.cursor).cloned() else {
                    return effects;
                };
                let card_id = card.card_id;
                match state.tts_states.get(&card_id) {
                    Some(TtsUiState::Synthesizing) => {
                        // Ignore repeat presses while synthesis is in flight.
                    }
                    Some(TtsUiState::Ready { cache_path }) => {
                        effects.push(Effect::PlayAudio {
                            card_id,
                            path: cache_path.clone(),
                        });
                    }
                    _ => {
                        // Idle or failed: ask the worker to synthesize
                        // from the current card snapshot. The worker
                        // never looks at any stale mirror.
                        //
                        // Mark `Synthesizing` optimistically so the
                        // Enter/Esc guards see the in-flight state on the
                        // very next key event — before the worker's
                        // `BackendEvent::TtsState::Synthesizing` reply
                        // round-trips. This is what blocks the
                        // press-p-then-Enter race on the same card.
                        state.tts_states.insert(card_id, TtsUiState::Synthesizing);
                        effects.push(Effect::TryPreviewTts { card_id, card });
                    }
                }
            }
            KeyCode::PageUp => {
                if let AppMode::Selecting(ref mut s) = self.mode {
                    s.detail_scroll = s.detail_scroll.saturating_sub(5);
                }
            }
            KeyCode::PageDown => {
                if let AppMode::Selecting(ref mut s) = self.mode {
                    s.detail_scroll += 5;
                }
            }
            _ => {}
        }

        effects
    }
}

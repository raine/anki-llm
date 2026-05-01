use std::sync::mpsc;

use crossterm::event::{Event, KeyCode, KeyModifiers};

use super::events::{TtsUiState, WorkerCommand};
use super::runner::any_card_synthesizing;
use super::state::{App, AppMode, Toast};

use crate::tui::line_input::LineInput;

impl App {
    pub(super) fn handle_key_selection(&mut self, key: crossterm::event::KeyEvent) {
        let AppMode::Selecting(ref mut state) = self.mode else {
            return;
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
                        self.worker_tx
                            .send(WorkerCommand::RegenerateCard { card, feedback })
                            .ok();
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
            return;
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
                            self.worker_tx.send(WorkerCommand::Cancel).ok();
                            self.pending_cancels += 1;
                            self.worker_tx.send(WorkerCommand::SetModel(model)).ok();
                            self.worker_tx
                                .send(WorkerCommand::Start {
                                    term,
                                    enable_thinking_stream: true,
                                })
                                .ok();
                        } else {
                            state.refresh_in_flight = true;
                            self.worker_tx
                                .send(WorkerCommand::RefreshWithTerm(term))
                                .ok();
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
            return;
        }

        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.worker_tx.send(WorkerCommand::Quit).ok();
                self.should_quit = true;
                self.user_quit = true;
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
                    self.worker_tx.send(WorkerCommand::Cancel).ok();
                    self.pending_cancels += 1;
                    self.worker_tx.send(WorkerCommand::SetModel(model)).ok();
                    let term = self.last_term.clone().unwrap_or_default();
                    self.worker_tx
                        .send(WorkerCommand::Start {
                            term,
                            enable_thinking_stream: true,
                        })
                        .ok();
                } else {
                    state.refresh_in_flight = true;
                    self.worker_tx.send(WorkerCommand::Refresh).ok();
                }
            }
            KeyCode::Char('t') if !state.refresh_in_flight => {
                state.term_input = Some(LineInput::default());
            }
            KeyCode::Char('e') => {
                self.pending_edit = Some(state.cursor);
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
                    return;
                }
                self.pending_model = None;
                self.batch_queue.clear();
                self.batch_progress = None;
                self.batch_cards.clear();
                self.worker_tx.send(WorkerCommand::Cancel).ok();
                self.pending_cancels += 1;
                self.reset_for_new_run();
                self.mode = AppMode::Input(LineInput::default());
            }
            KeyCode::Char('q') => {
                self.worker_tx.send(WorkerCommand::Quit).ok();
                self.should_quit = true;
                self.user_quit = true;
            }
            KeyCode::Enter if !state.refresh_in_flight => {
                // See the Esc arm for the race rationale; same guard.
                if any_card_synthesizing(state) {
                    self.toast = Some(Toast {
                        message: "TTS preview in progress".into(),
                        tick: self.tick,
                    });
                    return;
                }
                self.pending_model = None;
                let AppMode::Selecting(state) = std::mem::replace(&mut self.mode, AppMode::Running)
                else {
                    return;
                };
                let cards = state.selected_cards_in_order();
                let skip_post_select = state.skip_post_select;
                self.worker_tx
                    .send(WorkerCommand::Selection {
                        cards,
                        skip_post_select,
                    })
                    .ok();
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
                    self.copy_cards(&[card]);
                }
            }
            KeyCode::Char('p') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                // TTS preview: hidden keybind unless the session actually
                // supports it (prompt has `tts:` AND an audio player
                // was detected at startup).
                let enabled = self
                    .session_info
                    .as_ref()
                    .map(|info| info.tts_configured)
                    .unwrap_or(false);
                if !enabled || self.player.is_none() {
                    return;
                }
                let Some(card) = state.cards.get(state.cursor).cloned() else {
                    return;
                };
                let card_id = card.card_id;
                match state.tts_states.get(&card_id) {
                    Some(TtsUiState::Synthesizing) => {
                        // Ignore repeat presses while synthesis is in flight.
                    }
                    Some(TtsUiState::Ready { cache_path }) => {
                        // Already cached; tell the player directly. Same
                        // card id will toggle it off if still playing.
                        if let Some(player) = &self.player {
                            let _ = player.play(card_id, cache_path.clone());
                        }
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
                        // `try_send` instead of blocking `send`: the
                        // command channel is a bounded `sync_channel`
                        // and a full queue would otherwise block the
                        // render thread. On `Full`, roll the optimistic
                        // `Synthesizing` state back — otherwise the card
                        // would stay stuck forever (no worker reply will
                        // ever arrive) and `any_card_synthesizing` would
                        // permanently block Enter/Esc.
                        match self.worker_tx.try_send(WorkerCommand::PreviewTts { card }) {
                            Ok(()) => {}
                            Err(mpsc::TrySendError::Full(_)) => {
                                state.tts_states.remove(&card_id);
                                self.toast = Some(Toast {
                                    message: "Preview queue full — try again".into(),
                                    tick: self.tick,
                                });
                            }
                            Err(mpsc::TrySendError::Disconnected(_)) => {
                                state.tts_states.remove(&card_id);
                                self.mode =
                                    AppMode::Error("Worker thread exited unexpectedly".into());
                            }
                        }
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
    }
}

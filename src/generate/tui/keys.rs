use crossterm::event::{KeyCode, KeyModifiers};

use super::events::WorkerCommand;
use super::runner::done_audio_cache_path;
use super::state::{App, AppMode, Toast};

use crate::anki::client::anki_client;
use crate::tui::line_input::LineInput;

impl App {
    pub(super) fn handle_key(&mut self, key: crossterm::event::KeyEvent) {
        // Model picker overlay intercepts all keys when visible
        if let Some(ref mut picker) = self.model_picker {
            match key.code {
                KeyCode::Up => picker.move_up(),
                KeyCode::Down => picker.move_down(),
                KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    picker.move_down()
                }
                KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    picker.move_up()
                }
                KeyCode::Backspace => picker.remove_filter_char(),
                KeyCode::Char(c) => picker.add_filter_char(c),
                KeyCode::Enter => {
                    if let Some(model) = picker.selected() {
                        let changed = self
                            .session_info
                            .as_ref()
                            .map(|s| s.model != model)
                            .unwrap_or(true);
                        if changed {
                            if matches!(self.mode, AppMode::Selecting(_)) {
                                // In Selecting mode: defer the model change until
                                // the user requests more cards. The pipeline stays
                                // alive so Enter/Confirm still works normally.
                                self.pending_model = Some(model.clone());
                                if let Some(ref mut info) = self.session_info {
                                    info.model.clone_from(&model);
                                }
                                self.toast = Some(Toast {
                                    message: format!("Model: {model}"),
                                    tick: self.tick,
                                });
                            } else {
                                self.worker_tx.send(WorkerCommand::SetModel(model)).ok();
                            }
                        }
                    }
                    self.model_picker = None;
                }
                KeyCode::Esc => {
                    self.model_picker = None;
                }
                _ => {}
            }
            return;
        }

        // Help overlay intercepts all keys when visible
        if self.show_help {
            match key.code {
                KeyCode::Char('?') | KeyCode::Esc => self.show_help = false,
                _ => {}
            }
            return;
        }

        // Toggle help overlay from any mode (but not when typing in an inline input)
        let has_term_input = matches!(
            self.mode,
            AppMode::Selecting(ref s) if s.term_input.is_some()
        );
        if key.code == KeyCode::Char('?')
            && !matches!(self.mode, AppMode::Input(_))
            && !has_term_input
        {
            self.show_help = true;
            return;
        }

        match &mut self.mode {
            AppMode::Input(_) => self.handle_key_input(key),
            AppMode::Running => match key.code {
                KeyCode::Esc => {
                    // Cancel current run (and entire batch) and go back to term input.
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
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.worker_tx.send(WorkerCommand::Quit).ok();
                    self.should_quit = true;
                    self.user_quit = true;
                }
                _ => {}
            },
            AppMode::Selecting(_) => self.handle_key_selection(key),
            AppMode::Reviewing(_) => self.handle_key_review(key),
            AppMode::Done { .. } | AppMode::Error(_) => match key.code {
                KeyCode::Char('m')
                    if key.modifiers.contains(KeyModifiers::CONTROL) && !self.is_fatal =>
                {
                    self.open_model_picker();
                }
                KeyCode::Char('n') if !self.is_fatal => {
                    self.reset_for_new_run();
                    self.mode = AppMode::Input(LineInput::default());
                }
                KeyCode::Char('r') if !self.is_fatal => {
                    if let Some(term) = self.last_term.clone() {
                        self.reset_for_new_run();
                        self.mode = AppMode::Running;
                        self.worker_tx
                            .send(WorkerCommand::Start {
                                term,
                                enable_thinking_stream: true,
                            })
                            .ok();
                    }
                }
                KeyCode::Char('q') => {
                    self.worker_tx.send(WorkerCommand::Quit).ok();
                    self.should_quit = true;
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    if let Some(idx) = self.browse_step
                        && idx > 0
                    {
                        self.browse_step = Some(idx - 1);
                        self.browse_scroll = 0;
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if let Some(idx) = self.browse_step
                        && idx + 1 < self.steps.len()
                    {
                        self.browse_step = Some(idx + 1);
                        self.browse_scroll = 0;
                    }
                }
                KeyCode::PageUp => {
                    self.browse_scroll = self.browse_scroll.saturating_sub(10);
                }
                KeyCode::PageDown => {
                    self.browse_scroll += 10;
                }
                KeyCode::Char('c') => {
                    if let AppMode::Done { ref cards, .. } = self.mode {
                        let cards = cards.clone();
                        self.copy_cards(&cards);
                    }
                }
                KeyCode::Char('p') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    let enabled = self
                        .session_info
                        .as_ref()
                        .map(|info| info.tts_configured)
                        .unwrap_or(false);
                    if !enabled {
                        return;
                    }
                    let Some(player) = &self.player else {
                        return;
                    };
                    let Some(card) = (match &self.mode {
                        AppMode::Done { cards, .. } => cards.first(),
                        _ => None,
                    }) else {
                        return;
                    };
                    let Some(path) = done_audio_cache_path(card) else {
                        self.toast = Some(Toast {
                            message: "No cached audio to play".into(),
                            tick: self.tick,
                        });
                        return;
                    };
                    let _ = player.play(card.card_id, path);
                }
                KeyCode::Char('d') => {
                    if let AppMode::Done {
                        ref mut note_ids,
                        ref mut cards,
                        ref mut message,
                        ..
                    } = self.mode
                        && !note_ids.is_empty()
                    {
                        let anki = anki_client();
                        match anki.delete_notes(note_ids) {
                            Ok(()) => {
                                let count = note_ids.len();
                                note_ids.clear();
                                cards.clear();
                                *message = format!("Deleted {count} note(s) from Anki.");
                                self.toast = Some(Toast {
                                    message: format!("Deleted {count} note(s)"),
                                    tick: self.tick,
                                });
                            }
                            Err(e) => {
                                self.toast = Some(Toast {
                                    message: format!("Delete failed: {e}"),
                                    tick: self.tick,
                                });
                            }
                        }
                    }
                }
                _ => {}
            },
        }
    }

    pub(super) fn handle_key_review(&mut self, key: crossterm::event::KeyEvent) {
        let AppMode::Reviewing(ref mut state) = self.mode else {
            return;
        };

        match (key.code, key.modifiers) {
            (KeyCode::Char('k') | KeyCode::Char('y') | KeyCode::Enter, _) => {
                state.keep_current();
            }
            (KeyCode::Char('d') | KeyCode::Char('n'), KeyModifiers::NONE) => {
                state.discard_current();
            }
            (KeyCode::Char('u') | KeyCode::Backspace | KeyCode::Left, KeyModifiers::NONE) => {
                state.move_back();
            }
            (KeyCode::Char('a'), _) => {
                state.keep_all();
            }
            (KeyCode::Char('x'), _) => {
                state.discard_all();
            }
            (KeyCode::PageUp, _) | (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                state.detail_scroll = state.detail_scroll.saturating_sub(10);
            }
            (KeyCode::PageDown, _) | (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                state.detail_scroll += 10;
            }
            (KeyCode::Char('q'), _) => {
                self.worker_tx.send(WorkerCommand::Quit).ok();
                self.should_quit = true;
                self.user_quit = true;
                return;
            }
            _ => {}
        }

        // Check if review is complete
        if let AppMode::Reviewing(ref state) = self.mode
            && state.is_done()
        {
            let AppMode::Reviewing(state) = std::mem::replace(&mut self.mode, AppMode::Running)
            else {
                return;
            };
            let decisions = state.decisions.clone();
            self.worker_tx.send(WorkerCommand::Review(decisions)).ok();
        }
    }
}

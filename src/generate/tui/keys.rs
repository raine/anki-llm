use crossterm::event::{KeyCode, KeyModifiers};

use super::effects::Effect;
use super::events::WorkerCommand;
use super::runner::done_audio_cache_path;
use super::state::{App, AppMode, Toast};

use crate::tui::line_input::LineInput;

impl App {
    pub(super) fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> Vec<Effect> {
        let mut effects = Vec::new();

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
                                effects.push(Effect::SendWorker(WorkerCommand::SetModel(model)));
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
            return effects;
        }

        // Help overlay intercepts all keys when visible
        if self.show_help {
            match key.code {
                KeyCode::Char('?') | KeyCode::Esc => self.show_help = false,
                _ => {}
            }
            return effects;
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
            return effects;
        }

        match &mut self.mode {
            AppMode::Input(_) => effects.extend(self.handle_key_input(key)),
            AppMode::Running => match key.code {
                KeyCode::Esc => {
                    // Cancel current run (and entire batch) and go back to term input.
                    self.batch_queue.clear();
                    self.batch_progress = None;
                    self.batch_cards.clear();
                    effects.push(Effect::SendWorker(WorkerCommand::Cancel));
                    self.pending_cancels += 1;
                    self.reset_for_new_run();
                    self.mode = AppMode::Input(LineInput::default());
                }
                KeyCode::Char('q') => {
                    effects.push(Effect::SendWorker(WorkerCommand::Quit));
                    self.should_quit = true;
                    self.user_quit = true;
                }
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    effects.push(Effect::SendWorker(WorkerCommand::Quit));
                    self.should_quit = true;
                    self.user_quit = true;
                }
                _ => {}
            },
            AppMode::Selecting(_) => effects.extend(self.handle_key_selection(key)),
            AppMode::Reviewing(_) => effects.extend(self.handle_key_review(key)),
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
                        effects.push(Effect::SendWorker(WorkerCommand::Start {
                            term,
                            enable_thinking_stream: true,
                        }));
                    }
                }
                KeyCode::Char('q') => {
                    effects.push(Effect::SendWorker(WorkerCommand::Quit));
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
                        effects.push(Effect::CopyCards(cards.clone()));
                    }
                }
                KeyCode::Char('p') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    let enabled = self
                        .session_info
                        .as_ref()
                        .map(|info| info.tts_configured)
                        .unwrap_or(false);
                    if !enabled {
                        return effects;
                    }
                    if self.player.is_none() {
                        return effects;
                    }
                    let Some(card) = (match &self.mode {
                        AppMode::Done { cards, .. } => cards.first(),
                        _ => None,
                    }) else {
                        return effects;
                    };
                    let Some(path) = done_audio_cache_path(card) else {
                        self.toast = Some(Toast {
                            message: "No cached audio to play".into(),
                            tick: self.tick,
                        });
                        return effects;
                    };
                    effects.push(Effect::PlayAudio {
                        card_id: card.card_id,
                        path,
                    });
                }
                KeyCode::Char('d') => {
                    if let AppMode::Done { ref note_ids, .. } = self.mode
                        && !note_ids.is_empty()
                    {
                        effects.push(Effect::DeleteFromAnki {
                            note_ids: note_ids.clone(),
                        });
                    }
                }
                _ => {}
            },
        }

        effects
    }

    pub(super) fn handle_key_review(&mut self, key: crossterm::event::KeyEvent) -> Vec<Effect> {
        let mut effects = Vec::new();
        let AppMode::Reviewing(ref mut state) = self.mode else {
            return effects;
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
                effects.push(Effect::SendWorker(WorkerCommand::Quit));
                self.should_quit = true;
                self.user_quit = true;
                return effects;
            }
            _ => {}
        }

        // Check if review is complete
        if let AppMode::Reviewing(ref state) = self.mode
            && state.is_done()
        {
            let AppMode::Reviewing(state) = std::mem::replace(&mut self.mode, AppMode::Running)
            else {
                return effects;
            };
            let decisions = state.decisions.clone();
            effects.push(Effect::SendWorker(WorkerCommand::Review(decisions)));
        }

        effects
    }
}

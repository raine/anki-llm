use crossterm::event::{Event, KeyCode, KeyModifiers};

use super::effects::Effect;
use super::events::WorkerCommand;
use super::state::{App, AppMode};

use crate::tui::line_input::LineInput;

impl App {
    pub(super) fn handle_key_input(&mut self, key: crossterm::event::KeyEvent) -> Vec<Effect> {
        let mut effects = Vec::new();
        let AppMode::Input(ref mut input) = self.mode else {
            return effects;
        };

        match key.code {
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                effects.push(Effect::SwitchPrompt);
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                effects.push(Effect::Quit);
            }
            KeyCode::Esc => {
                if !input.value().is_empty() || !self.batch_queue.is_empty() {
                    input.reset();
                    self.batch_queue.clear();
                    self.history.reset_browse();
                }
            }
            KeyCode::Up => {
                if let Some(entry) = self.history.up(input.value()) {
                    let text = entry.to_string();
                    let len = text.chars().count();
                    *input = LineInput::new(text).with_cursor(len);
                }
            }
            KeyCode::Down => {
                if let Some(entry) = self.history.down() {
                    let text = entry.to_string();
                    let len = text.chars().count();
                    *input = LineInput::new(text).with_cursor(len);
                }
            }
            KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.open_model_picker();
            }
            KeyCode::Tab => {
                // Add current term to batch queue and clear input for next term
                let term = input.value().trim().to_string();
                if !term.is_empty() {
                    self.batch_queue.push(term);
                    input.reset();
                    self.history.reset_browse();
                }
            }
            KeyCode::Enter => {
                let term = input.value().trim().to_string();
                if !term.is_empty() {
                    self.history.reset_browse();

                    if self.batch_queue.is_empty() {
                        // Single term
                        self.history.push(&term);
                        self.last_term = Some(term.clone());
                        self.batch_progress = None;
                        self.mode = AppMode::Running;
                        effects.push(Effect::SendWorker(WorkerCommand::Start {
                            term,
                            enable_thinking_stream: true,
                        }));
                    } else {
                        // Batch: queue has earlier terms, input has the last one
                        self.batch_queue.push(term);
                        let total = self.batch_queue.len();
                        // Push all terms to history
                        for t in &self.batch_queue {
                            self.history.push(t);
                        }
                        let first = self.batch_queue.remove(0);
                        self.last_term = Some(first.clone());
                        self.batch_progress = Some((1, total));
                        self.mode = AppMode::Running;
                        effects.push(Effect::SendWorker(WorkerCommand::Start {
                            term: first,
                            enable_thinking_stream: false,
                        }));
                    }
                }
            }
            _ => {
                if input.handle_event(&Event::Key(key)) {
                    self.history.reset_browse();
                }
            }
        }
        effects
    }

    pub(super) fn handle_paste_input(&mut self, text: String) {
        match self.mode {
            AppMode::Input(ref mut input) => {
                // Detect multi-line paste: split into batch terms
                if text.contains('\n') || text.contains('\r') {
                    let terms: Vec<String> = text
                        .lines()
                        .map(|l| l.trim().to_string())
                        .filter(|l| !l.is_empty())
                        .collect();

                    if terms.len() > 1 {
                        // Deduplicate preserving order
                        let mut seen = std::collections::HashSet::new();
                        let terms: Vec<String> = terms
                            .into_iter()
                            .filter(|t| seen.insert(t.clone()))
                            .collect();

                        // Put first term in the input, rest in batch_queue
                        *input = LineInput::new(terms[0].clone());
                        self.batch_queue = terms[1..].to_vec();
                        self.history.reset_browse();
                        return;
                    } else if terms.len() == 1 {
                        // Single non-empty line after trimming
                        *input = LineInput::new(terms[0].clone());
                        self.batch_queue.clear();
                        self.history.reset_browse();
                        return;
                    }
                }

                if input.handle_event(&Event::Paste(text)) {
                    self.history.reset_browse();
                }
            }
            AppMode::Selecting(ref mut state) => {
                if let Some(ref mut input) = state.term_input {
                    input.handle_event(&Event::Paste(text));
                }
            }
            _ => {}
        }
    }
}

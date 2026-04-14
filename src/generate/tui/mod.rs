use std::sync::mpsc;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, Wrap};
use ratatui::{DefaultTerminal, Frame};

pub(crate) mod events;
mod history;
mod prompt_picker;
mod screens;
mod widgets;

pub use events::{BackendEvent, SessionInfo, StepStatus, TtsUiState, WorkerCommand};
use history::InputHistory;
use screens::review::{ReviewState, draw_reviewing};
use screens::selection::{SelectionState, draw_selecting};
use widgets::{ModelPickerState, draw_log_panel, draw_model_picker, draw_step_logs};

use crate::tui::line_input::LineInput;
use crate::tui::theme::{Glyphs, SPINNER_FRAMES, THEME, footer_cmd, footer_pipe};

use crate::anki::client::AnkiClient;
use crate::cli::GenerateArgs;
use crate::llm::pricing;

use super::cards::ValidatedCard;

// Re-export PipelineStep from the shared pipeline module
pub use super::pipeline::PipelineStep;

const ALL_STEPS: &[PipelineStep] = &[
    PipelineStep::LoadPrompt,
    PipelineStep::ValidateAnki,
    PipelineStep::Generate,
    PipelineStep::PostProcess,
    PipelineStep::Validate,
    PipelineStep::Select,
    PipelineStep::QualityCheck,
    PipelineStep::Finish,
];

struct StepRecord {
    step: PipelineStep,
    status: StepStatus,
    logs: Vec<String>,
}

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

enum AppMode {
    Input(LineInput), // term text being typed
    Running,
    Selecting(SelectionState),
    Reviewing(ReviewState),
    Done {
        message: String,
        cards: Vec<ValidatedCard>,
        note_ids: Vec<i64>,
        /// When true, the run finished with a non-fatal failure and the
        /// summary header should render in an error style. Cards are
        /// still shown so the user can copy out work they had curated
        /// before the failure.
        failed: bool,
    },
    Error(String),
}

struct App {
    mode: AppMode,
    session_info: Option<SessionInfo>,
    logs: Vec<String>,
    steps: Vec<StepRecord>,
    /// Index of the currently-running step (for bucketing logs).
    current_step_idx: Option<usize>,
    /// Cost for the current run.
    run_cost: f64,
    run_input_tokens: u64,
    run_output_tokens: u64,
    /// Accumulated cost across all runs in this session.
    session_cost: f64,
    log_scroll: u16,
    log_auto_scroll: bool,
    tick: u64,
    /// Counts how many runs have been cancelled. While > 0, backend events are
    /// discarded. Decremented when RunDone/RunError arrives from a cancelled run.
    pending_cancels: u32,
    should_quit: bool,
    /// True when the user explicitly pressed q/Ctrl-C (as opposed to natural Done/Error exit).
    user_quit: bool,
    /// True when the user pressed Ctrl+P to switch prompt.
    switch_prompt: bool,
    show_help: bool,
    model_picker: Option<ModelPickerState>,
    /// Last term submitted, for retry.
    last_term: Option<String>,
    /// Model name to apply before the next pipeline run (deferred model change).
    pending_model: Option<String>,
    /// True after a Fatal error — worker is dead, no new runs possible.
    is_fatal: bool,
    glyphs: Glyphs,
    history: InputHistory,
    toast: Option<Toast>,
    /// In Done/Error mode: selected step index for log browsing, None = summary.
    browse_step: Option<usize>,
    browse_scroll: u16,
    /// When Some, the main loop should suspend the TUI and open $EDITOR for this card index.
    pending_edit: Option<usize>,
    /// Remaining terms to process in a batch (front = next term).
    batch_queue: Vec<String>,
    /// Batch progress: (current 1-based index, total count). None when not in batch.
    batch_progress: Option<(usize, usize)>,
    /// Accumulated cards during batch processing (before entering selection).
    batch_cards: Vec<ValidatedCard>,
    backend_rx: mpsc::Receiver<BackendEvent>,
    worker_tx: mpsc::SyncSender<WorkerCommand>,
    /// Audio playback thread handle. `Some` when a system player was
    /// detected at session startup AND the prompt has a `tts:` block;
    /// the preview keybind is hidden and ignored when `None`.
    player: Option<crate::audio::PlayerHandle>,
    /// Remembered binary discovered at startup. Retained so the player
    /// could be lazily respawned later if needed; currently unused but
    /// kept alongside the handle for symmetry.
    player_binary: Option<crate::audio::PlayerBinary>,
}

struct Toast {
    message: String,
    tick: u64,
}

impl App {
    fn new(
        initial_term: Option<String>,
        glyphs: Glyphs,
        backend_rx: mpsc::Receiver<BackendEvent>,
        worker_tx: mpsc::SyncSender<WorkerCommand>,
    ) -> Self {
        let steps = ALL_STEPS
            .iter()
            .map(|&s| StepRecord {
                step: s,
                status: StepStatus::Pending,
                logs: Vec::new(),
            })
            .collect();
        let last_term = initial_term.clone();
        let mode = if let Some(term) = initial_term {
            worker_tx.send(WorkerCommand::Start(term)).ok();
            AppMode::Running
        } else {
            AppMode::Input(LineInput::default())
        };
        App {
            mode,
            session_info: None,
            logs: Vec::new(),
            steps,
            current_step_idx: None,
            run_cost: 0.0,
            run_input_tokens: 0,
            run_output_tokens: 0,
            session_cost: 0.0,
            log_scroll: 0,
            log_auto_scroll: true,
            tick: 0,
            pending_cancels: 0,
            should_quit: false,
            user_quit: false,
            switch_prompt: false,
            show_help: false,
            model_picker: None,
            last_term,
            pending_model: None,
            is_fatal: false,
            glyphs,
            history: InputHistory::load(),
            toast: None,
            browse_step: None,
            browse_scroll: 0,
            pending_edit: None,
            batch_queue: Vec::new(),
            batch_progress: None,
            batch_cards: Vec::new(),
            backend_rx,
            worker_tx,
            player: None,
            player_binary: crate::audio::detect_player_binary(),
        }
    }

    fn reset_for_new_run(&mut self) {
        self.logs.clear();
        self.log_scroll = 0;
        self.log_auto_scroll = true;
        self.session_cost += self.run_cost;
        self.run_cost = 0.0;
        self.run_input_tokens = 0;
        self.run_output_tokens = 0;
        for record in &mut self.steps {
            record.status = StepStatus::Pending;
            record.logs.clear();
        }
        self.current_step_idx = None;
        self.browse_step = None;
        self.browse_scroll = 0;
    }

    fn copy_cards(&mut self, cards: &[ValidatedCard]) {
        if cards.is_empty() {
            return;
        }
        let text = cards
            .iter()
            .map(|card| {
                card.raw_anki_fields
                    .iter()
                    .map(|(name, value)| {
                        let plain = super::selector::strip_html_tags(value);
                        format!("{name}\n{plain}")
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n")
            })
            .collect::<Vec<_>>()
            .join("\n\n────────────────────────────────────────\n\n");
        if let Ok(mut cb) = arboard::Clipboard::new() {
            cb.set_text(text).ok();
        }
        self.toast = Some(Toast {
            message: "Copied!".into(),
            tick: self.tick,
        });
    }

    fn open_model_picker(&mut self) {
        if let Some(info) = &self.session_info
            && !info.available_models.is_empty()
        {
            self.model_picker = Some(ModelPickerState::new(
                info.available_models.clone(),
                Some(info.model.as_str()),
            ));
        }
    }

    fn step_index(&self, step: PipelineStep) -> Option<usize> {
        self.steps.iter().position(|r| r.step == step)
    }

    fn step_status_mut(&mut self, step: PipelineStep) -> Option<&mut StepStatus> {
        self.steps
            .iter_mut()
            .find(|r| r.step == step)
            .map(|r| &mut r.status)
    }

    fn handle_backend_event(&mut self, event: BackendEvent) {
        // SessionReady is always relevant
        if let BackendEvent::SessionReady(info) = event {
            // Lazy-init the audio player the first time we see a session
            // where TTS preview is live (frontmatter has a `tts:` block
            // AND a playback binary was found at startup).
            if info.tts_configured && self.player.is_none() {
                if let Some(bin) = self.player_binary.clone() {
                    self.player = Some(crate::audio::spawn_player(bin));
                } else {
                    self.logs
                        .push("Audio player not found — preview disabled".into());
                }
            }
            self.session_info = Some(info);
            return;
        }

        // Discard events from abandoned runs. Each cancelled run will eventually
        // produce a RunDone or RunError; decrement the counter when that arrives.
        if self.pending_cancels > 0 {
            if matches!(
                event,
                BackendEvent::RunDone { .. } | BackendEvent::RunError(_)
            ) {
                self.pending_cancels -= 1;
            }
            return;
        }

        match event {
            BackendEvent::SessionReady(_) => unreachable!(),
            BackendEvent::Log(msg) => {
                if let Some(idx) = self.current_step_idx {
                    self.steps[idx].logs.push(msg.clone());
                }
                self.logs.push(msg);
                if self.log_auto_scroll {
                    self.log_scroll = self.logs.len().saturating_sub(1) as u16;
                }
            }
            BackendEvent::StepUpdate { step, status } => {
                if matches!(status, StepStatus::Running(_)) {
                    self.current_step_idx = self.step_index(step);
                }
                if let Some(st) = self.step_status_mut(step) {
                    *st = status;
                }
            }
            BackendEvent::RequestSelection(cards) => {
                if let AppMode::Selecting(ref mut state) = self.mode {
                    // Already selecting (model-change refresh): append new cards
                    state.cards.extend(cards);
                    state.refresh_in_flight = false;
                } else if self.batch_queue.is_empty() && self.batch_cards.is_empty() {
                    // Single term or last batch term (first result): go to selection
                    self.batch_progress = None;
                    self.mode = AppMode::Selecting(SelectionState::new(cards));
                } else if !self.batch_queue.is_empty() {
                    // Batch: accumulate cards, stay in Running, advance to next term
                    self.batch_cards.extend(cards);
                    let next_term = self.batch_queue.remove(0);
                    if let Some((ref mut current, _)) = self.batch_progress {
                        *current += 1;
                    }
                    self.last_term = Some(next_term.clone());
                    self.worker_tx
                        .send(WorkerCommand::RefreshWithTerm(next_term))
                        .ok();
                } else {
                    // batch_queue empty but batch_cards non-empty: handle gracefully
                    let mut all_cards = std::mem::take(&mut self.batch_cards);
                    all_cards.extend(cards);
                    self.batch_progress = None;
                    self.mode = AppMode::Selecting(SelectionState::new(all_cards));
                }
            }
            BackendEvent::AppendCards(new_cards) => {
                if !self.batch_cards.is_empty() || !self.batch_queue.is_empty() {
                    // Still in batch processing (Running mode): accumulate
                    self.batch_cards.extend(new_cards);
                    if let Some(next_term) = self.batch_queue.first().cloned() {
                        self.batch_queue.remove(0);
                        if let Some((ref mut current, _)) = self.batch_progress {
                            *current += 1;
                        }
                        self.last_term = Some(next_term.clone());
                        self.worker_tx
                            .send(WorkerCommand::RefreshWithTerm(next_term))
                            .ok();
                    } else {
                        // Last batch term done: enter selection with all cards
                        let all_cards = std::mem::take(&mut self.batch_cards);
                        self.batch_progress = None;
                        self.mode = AppMode::Selecting(SelectionState::new(all_cards));
                    }
                } else if let AppMode::Selecting(ref mut state) = self.mode {
                    // Non-batch refresh (manual 'r' or 't'): append as before
                    state.cards.extend(new_cards);
                    state.refresh_in_flight = false;
                }
            }
            BackendEvent::ReplaceCard {
                previous_card_id,
                card,
            } => {
                if let AppMode::Selecting(ref mut state) = self.mode {
                    // Look up the row by stable id, not index. If the
                    // user removed or edited the card while regen was
                    // in flight, the reply has nothing to attach to —
                    // drop it silently.
                    let Some(slot) = state
                        .cards
                        .iter_mut()
                        .find(|c| c.card_id == previous_card_id)
                    else {
                        if state.regen_in_flight == Some(previous_card_id) {
                            state.regen_in_flight = None;
                        }
                        return;
                    };
                    let was_selected = state.selected.remove(&previous_card_id);
                    state.tts_states.remove(&previous_card_id);
                    let new_id = card.card_id;
                    *slot = card;
                    if was_selected {
                        state.selected.insert(new_id);
                    }
                    if state.regen_in_flight == Some(previous_card_id) {
                        state.regen_in_flight = None;
                    }
                    self.toast = Some(Toast {
                        message: "Card regenerated".into(),
                        tick: self.tick,
                    });
                }
            }
            BackendEvent::RegenError { target_id, message } => {
                if let AppMode::Selecting(ref mut state) = self.mode {
                    // Only clear the spinner if THIS target is still
                    // the in-flight one. A late error for an orphaned
                    // (edited / removed) card must not stomp on a
                    // different card's regen-in-flight state.
                    if state.regen_in_flight == Some(target_id) {
                        state.regen_in_flight = None;
                    }
                }
                self.toast = Some(Toast {
                    message,
                    tick: self.tick,
                });
            }
            BackendEvent::TtsState { card_id, state } => {
                if let AppMode::Selecting(ref mut sel) = self.mode {
                    // Drop replies for cards that were removed or
                    // edited (and thus had their `card_id` re-minted)
                    // while synthesis was in flight. Without this
                    // gate, a stale `Ready` reply would auto-play
                    // pre-edit audio once and leak an orphaned entry
                    // into `tts_states`.
                    if !sel.cards.iter().any(|c| c.card_id == card_id) {
                        return;
                    }
                    // On successful synth, immediately route a Play
                    // command to the audio thread. The player itself
                    // handles toggle-on-same-card semantics, so there's
                    // no TUI-side state machine to keep coherent.
                    if let TtsUiState::Ready { ref cache_path } = state
                        && let Some(player) = &self.player
                    {
                        let _ = player.play(card_id, cache_path.clone());
                    }
                    sel.tts_states.insert(card_id, state);
                }
            }
            BackendEvent::RequestReview(flagged) => {
                self.mode = AppMode::Reviewing(ReviewState::new(flagged));
            }
            BackendEvent::CostUpdate {
                input_tokens,
                output_tokens,
                cost,
            } => {
                self.run_input_tokens += input_tokens;
                self.run_output_tokens += output_tokens;
                self.run_cost += cost;
            }
            BackendEvent::RunDone {
                message,
                cards,
                note_ids,
                failed,
            } => {
                self.mode = AppMode::Done {
                    message,
                    cards,
                    note_ids,
                    failed,
                };
                self.current_step_idx = None;
            }
            BackendEvent::RunError(msg) => {
                // During batch: log the error and try to continue with next term
                if !self.batch_queue.is_empty() {
                    let failed_term = self.last_term.as_deref().unwrap_or("?");
                    self.toast = Some(Toast {
                        message: format!("Failed: {failed_term}"),
                        tick: self.tick,
                    });
                    self.logs
                        .push(format!("Error for term \"{failed_term}\": {msg}"));

                    let next_term = self.batch_queue.remove(0);
                    if let Some((ref mut current, _)) = self.batch_progress {
                        *current += 1;
                    }
                    self.last_term = Some(next_term.clone());
                    // Reset step indicators but keep logs for batch continuity
                    for record in &mut self.steps {
                        record.status = StepStatus::Pending;
                        record.logs.clear();
                    }
                    self.current_step_idx = None;
                    self.mode = AppMode::Running;
                    self.worker_tx.send(WorkerCommand::Start(next_term)).ok();
                } else {
                    self.batch_progress = None;
                    // If we accumulated cards before this error, show selection
                    if !self.batch_cards.is_empty() {
                        let all_cards = std::mem::take(&mut self.batch_cards);
                        self.mode = AppMode::Selecting(SelectionState::new(all_cards));
                    } else {
                        self.mode = AppMode::Error(msg);
                    }
                }
            }
            BackendEvent::ModelChangeError(msg) => {
                self.logs.push(format!("Model change failed: {msg}"));
            }
            BackendEvent::Fatal(msg) => {
                // Mark the currently-running step as failed so the spinner
                // is replaced with the ✗ icon.
                for record in &mut self.steps {
                    if matches!(record.status, StepStatus::Running(_)) {
                        record.status = StepStatus::Error(msg.clone());
                        break;
                    }
                }
                self.mode = AppMode::Error(msg);
                self.is_fatal = true;
            }
        }
    }

    fn handle_key(&mut self, key: crossterm::event::KeyEvent) {
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
                KeyCode::Up | KeyCode::Char('k') => {
                    self.log_scroll = self.log_scroll.saturating_sub(1);
                    self.log_auto_scroll = false;
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.log_scroll =
                        (self.log_scroll + 1).min(self.logs.len().saturating_sub(1) as u16);
                    // Re-enable auto-scroll if at the bottom
                    if self.log_scroll as usize >= self.logs.len().saturating_sub(1) {
                        self.log_auto_scroll = true;
                    }
                }
                KeyCode::PageUp => {
                    self.log_scroll = self.log_scroll.saturating_sub(10);
                    self.log_auto_scroll = false;
                }
                KeyCode::PageDown => {
                    self.log_scroll =
                        (self.log_scroll + 10).min(self.logs.len().saturating_sub(1) as u16);
                    if self.log_scroll as usize >= self.logs.len().saturating_sub(1) {
                        self.log_auto_scroll = true;
                    }
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
                        self.worker_tx.send(WorkerCommand::Start(term)).ok();
                    }
                }
                KeyCode::Char('q') => {
                    self.worker_tx.send(WorkerCommand::Quit).ok();
                    self.should_quit = true;
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    if let Some(idx) = self.browse_step {
                        if idx > 0 {
                            self.browse_step = Some(idx - 1);
                            self.browse_scroll = 0;
                        } else {
                            self.browse_step = None;
                            self.browse_scroll = 0;
                        }
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => match self.browse_step {
                    None => {
                        self.browse_step = Some(0);
                        self.browse_scroll = 0;
                    }
                    Some(idx) if idx + 1 < self.steps.len() => {
                        self.browse_step = Some(idx + 1);
                        self.browse_scroll = 0;
                    }
                    _ => {}
                },
                KeyCode::PageUp => {
                    self.browse_scroll = self.browse_scroll.saturating_sub(10);
                }
                KeyCode::PageDown => {
                    if self.browse_step.is_some() {
                        self.browse_scroll += 10;
                    }
                }
                KeyCode::Esc => {
                    if self.browse_step.is_some() {
                        self.browse_step = None;
                        self.browse_scroll = 0;
                    }
                }
                KeyCode::Char('c') => {
                    if let AppMode::Done { ref cards, .. } = self.mode {
                        let cards = cards.clone();
                        self.copy_cards(&cards);
                    }
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
                        let anki = AnkiClient::new();
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

    fn handle_key_input(&mut self, key: crossterm::event::KeyEvent) {
        let AppMode::Input(ref mut input) = self.mode else {
            return;
        };

        match key.code {
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.worker_tx.send(WorkerCommand::Quit).ok();
                self.should_quit = true;
                self.switch_prompt = true;
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.worker_tx.send(WorkerCommand::Quit).ok();
                self.should_quit = true;
                self.user_quit = true;
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
                        self.worker_tx.send(WorkerCommand::Start(term)).ok();
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
                        self.worker_tx.send(WorkerCommand::Start(first)).ok();
                    }
                }
            }
            _ => {
                if input.handle_event(&Event::Key(key)) {
                    self.history.reset_browse();
                }
            }
        }
    }

    fn handle_paste_input(&mut self, text: String) {
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

    fn handle_key_selection(&mut self, key: crossterm::event::KeyEvent) {
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
                            self.worker_tx.send(WorkerCommand::Start(term)).ok();
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
                    self.worker_tx.send(WorkerCommand::Start(term)).ok();
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
                self.worker_tx.send(WorkerCommand::Selection(cards)).ok();
            }
            KeyCode::Char('f') => {
                state.force_toggle_duplicate();
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
                        self.worker_tx.send(WorkerCommand::PreviewTts { card }).ok();
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

    fn handle_key_review(&mut self, key: crossterm::event::KeyEvent) {
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

// ---------------------------------------------------------------------------
// External editor
// ---------------------------------------------------------------------------

/// Suspend the TUI, open the focused card in $EDITOR, and apply edits.
fn edit_card_in_editor(terminal: &mut DefaultTerminal, app: &mut App, card_index: usize) {
    let AppMode::Selecting(ref state) = app.mode else {
        return;
    };
    let Some(card) = state.cards.get(card_index) else {
        return;
    };
    let Some(ref info) = app.session_info else {
        return;
    };

    // Build ordered YAML from raw_anki_fields (Anki field names → raw markdown).
    // We use a Vec of (key, value) to preserve field order via serde_yaml.
    let fields_for_edit: indexmap::IndexMap<String, String> = card.raw_anki_fields.clone();
    let yaml = match serde_yaml::to_string(&fields_for_edit) {
        Ok(y) => y,
        Err(e) => {
            app.toast = Some(Toast {
                message: format!("Failed to serialize: {e}"),
                tick: app.tick,
            });
            return;
        }
    };

    // Write to temp file
    let tmp_dir = std::env::temp_dir();
    let tmp_path = tmp_dir.join("anki-llm-edit.yaml");
    if std::fs::write(&tmp_path, &yaml).is_err() {
        app.toast = Some(Toast {
            message: "Failed to write temp file".into(),
            tick: app.tick,
        });
        return;
    }

    // Determine editor
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "vi".to_string());

    // Suspend TUI
    crossterm::execute!(std::io::stdout(), crossterm::event::DisableBracketedPaste).ok();
    ratatui::restore();

    // Spawn editor
    let status = std::process::Command::new(&editor).arg(&tmp_path).status();

    // Resume TUI
    *terminal = ratatui::init();
    crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste).ok();

    let ok = match status {
        Ok(s) if s.success() => true,
        Ok(_) => {
            app.toast = Some(Toast {
                message: "Editor exited with error".into(),
                tick: app.tick,
            });
            false
        }
        Err(e) => {
            app.toast = Some(Toast {
                message: format!("Failed to launch {editor}: {e}"),
                tick: app.tick,
            });
            false
        }
    };

    if !ok {
        let _ = std::fs::remove_file(&tmp_path);
        return;
    }

    // Read edited content
    let edited_yaml = match std::fs::read_to_string(&tmp_path) {
        Ok(s) => s,
        Err(e) => {
            app.toast = Some(Toast {
                message: format!("Failed to read edited file: {e}"),
                tick: app.tick,
            });
            return;
        }
    };
    let _ = std::fs::remove_file(&tmp_path);

    // Parse edited YAML (Anki field names → raw markdown)
    let edited_anki_fields: indexmap::IndexMap<String, String> =
        match parse_edited_anki_fields(&edited_yaml) {
            Ok(m) => m,
            Err(e) => {
                app.toast = Some(Toast {
                    message: format!("YAML parse error: {e}"),
                    tick: app.tick,
                });
                return;
            }
        };

    // Build reverse map: Anki name → LLM key
    let reverse_map: std::collections::HashMap<&str, &str> = info
        .field_map
        .iter()
        .map(|(llm_key, anki_name)| (anki_name.as_str(), llm_key.as_str()))
        .collect();

    // Rebuild fields (LLM keys → sanitized HTML) and raw_anki_fields
    let mut new_fields: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut new_raw_anki_fields: indexmap::IndexMap<String, String> = indexmap::IndexMap::new();
    let mut new_anki_fields: indexmap::IndexMap<String, String> = indexmap::IndexMap::new();

    for (anki_name, raw_value) in &edited_anki_fields {
        new_raw_anki_fields.insert(anki_name.clone(), raw_value.clone());
        let sanitized = super::sanitize::sanitize_html(raw_value);
        new_anki_fields.insert(anki_name.clone(), sanitized.clone());
        if let Some(&llm_key) = reverse_map.get(anki_name.as_str()) {
            new_fields.insert(llm_key.to_string(), sanitized);
        }
    }

    // Re-check duplicate status against the authoritative first-field
    // name from `SessionInfo` — `new_anki_fields` preserves whatever
    // order the user wrote in `$EDITOR`, so trusting its insertion
    // order would query Anki against the wrong field whenever the user
    // rearranged the YAML. Refresh the full duplicate metadata shape
    // (note id + fields) via the shared helper so the selection
    // screen's diff panel renders against up-to-date data rather than
    // the pre-edit (or stale) existing-note fields.
    let first_field_value = new_anki_fields
        .get(&info.first_field_name)
        .cloned()
        .unwrap_or_default();
    let (new_dup_note_id, new_duplicate_fields) = {
        let anki = AnkiClient::new();
        super::cards::lookup_duplicate_metadata(
            &anki,
            &first_field_value,
            &info.note_type,
            &info.deck,
        )
        .unwrap_or((None, None))
    };
    let is_duplicate = new_dup_note_id.is_some();

    // Apply edits to the card. Mint a new `card_id` so any stale TTS
    // preview state (cached `Ready` path pointing at pre-edit audio,
    // or an in-flight `Synthesizing` reply) is invalidated by id
    // mismatch. Transfer selection/regen-flight membership from the
    // old id to the new one.
    let AppMode::Selecting(ref mut state) = app.mode else {
        return;
    };
    if let Some(card) = state.cards.get_mut(card_index) {
        let old_id = card.card_id;
        let new_id = crate::generate::cards::next_card_id();
        card.card_id = new_id;
        card.fields = new_fields;
        card.anki_fields = new_anki_fields;
        card.raw_anki_fields = new_raw_anki_fields;
        card.is_duplicate = is_duplicate;
        card.duplicate_note_id = new_dup_note_id;
        card.duplicate_fields = new_duplicate_fields;
        card.flags.clear(); // clear stale flags after manual edit

        if state.selected.remove(&old_id) {
            state.selected.insert(new_id);
        }
        // Editing semantically *cancels* an in-flight regeneration:
        // the worker is generating against the pre-edit text, so its
        // reply is no longer relevant. Clear the spinner now; the
        // late reply will be tagged with the old id and dropped on
        // arrival by `ReplaceCard`'s `iter_mut().find` lookup.
        if state.regen_in_flight == Some(old_id) {
            state.regen_in_flight = None;
        }
        state.tts_states.remove(&old_id);

        app.toast = Some(Toast {
            message: "Card updated".into(),
            tick: app.tick,
        });
    }
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();

    // Persistent shell: sidebar | main content, with footer below
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(area);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(30), Constraint::Min(0)])
        .split(rows[0]);

    // Render main content first, then sidebar on top so CJK bleed is covered
    let main_area = cols[1];
    match &app.mode {
        AppMode::Input(input) => draw_input(
            frame,
            input,
            app.history.browse_position(),
            app.batch_queue.len(),
            main_area,
            app.model_picker.is_none(),
        ),
        AppMode::Running => draw_running(frame, app, main_area),
        AppMode::Selecting(state) => draw_selecting(frame, state, &app.glyphs, app.tick, main_area),
        AppMode::Reviewing(state) => draw_reviewing(frame, state, main_area),
        AppMode::Done {
            message,
            cards,
            failed,
            ..
        } => {
            if let Some(step_idx) = app.browse_step {
                let record = &app.steps[step_idx];
                draw_step_logs(
                    frame,
                    record.step.label(),
                    &record.logs,
                    app.browse_scroll,
                    main_area,
                );
            } else {
                draw_done(frame, app, message, cards, *failed, main_area);
            }
        }
        AppMode::Error(msg) => {
            if let Some(step_idx) = app.browse_step {
                let record = &app.steps[step_idx];
                draw_step_logs(
                    frame,
                    record.step.label(),
                    &record.logs,
                    app.browse_scroll,
                    main_area,
                );
            } else {
                draw_error(frame, msg, main_area);
            }
        }
    }

    draw_sidebar(frame, app, cols[0]);
    draw_footer(frame, app, rows[1]);

    // Toast notification (e.g. "Copied!")
    if let Some(ref toast) = app.toast
        && app.tick.wrapping_sub(toast.tick) < 20
    {
        let text = &toast.message;
        let width = (text.len() as u16) + 2; // 1 padding each side
        let toast_area = Rect {
            x: main_area.x + 1,
            y: main_area.y + main_area.height.saturating_sub(2),
            width: width.min(main_area.width),
            height: 1,
        };
        let para = Paragraph::new(Span::styled(
            format!(" {text} "),
            Style::default()
                .fg(THEME.success)
                .add_modifier(Modifier::BOLD),
        ));
        frame.render_widget(Clear, toast_area);
        frame.render_widget(para, toast_area);
    }

    if app.show_help {
        draw_help_overlay(frame, app);
    }

    if let Some(picker) = &app.model_picker {
        draw_model_picker(frame, picker);
    }
}

fn draw_help_overlay(frame: &mut Frame, app: &App) {
    let shortcuts: Vec<(&str, &str)> = match &app.mode {
        AppMode::Input(_) => vec![
            ("Enter", "Generate"),
            ("Tab", "Queue term"),
            ("Ctrl+O", "Model"),
            ("↑ / ↓", "History"),
            ("Ctrl+P", "Switch prompt"),
            ("Esc", "Clear"),
            ("Ctrl+C", "Quit"),
        ],
        AppMode::Running => vec![
            ("j / k", "Scroll log"),
            ("PgUp/PgDn", "Scroll log fast"),
            ("Esc", "Cancel"),
            ("q", "Quit"),
        ],
        AppMode::Selecting(_) => {
            let mut v = vec![
                ("Space", "Toggle"),
                ("f", "Force-select duplicate"),
                ("a", "All"),
                ("n", "None"),
                ("c", "Copy"),
                ("d", "Remove"),
                ("e", "Edit in $EDITOR"),
                ("r", "More"),
                ("t", "More (new term)"),
                ("R", "Regenerate card"),
            ];
            if app
                .session_info
                .as_ref()
                .map(|info| info.tts_configured)
                .unwrap_or(false)
                && app.player.is_some()
            {
                v.push(("p", "Preview audio"));
            }
            v.extend([
                ("Ctrl+O", "Model"),
                ("Enter", "Confirm"),
                ("Esc", "Back"),
                ("q", "Quit"),
                ("PgUp/PgDn", "Scroll"),
            ]);
            v
        }
        AppMode::Reviewing(_) => vec![
            ("k / y", "Keep"),
            ("d / n", "Discard"),
            ("a", "Keep all"),
            ("x", "Discard all"),
            ("u", "Back"),
            ("q", "Quit"),
        ],
        AppMode::Done { note_ids, .. } => {
            let mut v = vec![
                ("j / k", "Browse steps"),
                ("PgUp/PgDn", "Scroll logs"),
                ("Esc", "Back to summary"),
                ("n", "New term"),
                ("r", "Retry"),
                ("Ctrl+O", "Model"),
            ];
            if !note_ids.is_empty() {
                v.push(("d", "Delete from Anki"));
            }
            v.push(("q", "Quit"));
            v
        }
        AppMode::Error(_) => {
            vec![
                ("j / k", "Browse steps"),
                ("PgUp/PgDn", "Scroll logs"),
                ("Esc", "Back to summary"),
                ("n", "New term"),
                ("r", "Retry"),
                ("Ctrl+O", "Model"),
                ("q", "Quit"),
            ]
        }
    };

    let row_count = shortcuts.len() as u16;
    let height = row_count + 5; // borders + padding + empty line at top
    let width: u16 = 44;

    let area = frame.area();
    let rect = Rect::new(
        area.width.saturating_sub(width) / 2,
        area.height.saturating_sub(height) / 2,
        width.min(area.width),
        height.min(area.height),
    );

    let mode_title = match &app.mode {
        AppMode::Input(_) => "Input",
        AppMode::Running => "Running",
        AppMode::Selecting(_) => "Select",
        AppMode::Reviewing(_) => "Review",
        AppMode::Done { .. } => "Done",
        AppMode::Error(_) => "Error",
    };

    let block = Block::bordered()
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(THEME.help_border))
        .title(Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled(
                mode_title,
                Style::default()
                    .fg(THEME.header)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ", Style::default()),
        ]))
        .title_bottom(Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled("any key", Style::default().fg(THEME.dimmed)),
            Span::styled(" to close ", Style::default().fg(THEME.help_muted)),
        ]));

    let mut rows: Vec<Row> = vec![Row::new(vec![Cell::from(""), Cell::from("")])];
    rows.extend(shortcuts.into_iter().map(|(key, desc)| {
        Row::new(vec![
            Cell::from(Line::from(vec![
                Span::styled(" ", Style::default()),
                Span::styled(
                    format!("{:>8}", key),
                    Style::default()
                        .fg(THEME.dimmed)
                        .add_modifier(Modifier::BOLD),
                ),
            ])),
            Cell::from(Line::from(vec![
                Span::styled(" · ", Style::default().fg(THEME.help_muted)),
                Span::styled(desc, Style::default().fg(THEME.text)),
            ])),
        ])
    }));

    let table = Table::new(rows, [Constraint::Length(10), Constraint::Min(25)])
        .block(block)
        .column_spacing(0);

    frame.render_widget(Clear, rect);
    frame.render_widget(table, rect);
}

fn draw_sidebar(frame: &mut Frame, app: &App, area: Rect) {
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(THEME.border));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let has_term = app.last_term.is_some() && !matches!(app.mode, AppMode::Input(_));
    let info_height: u16 = match (app.session_info.is_some(), has_term) {
        (true, true) => 5,
        (true, false) => 4,
        _ => 0,
    };

    // Build step lines (detail on second indented line)
    let spinner_frame = format!(
        "{} ",
        SPINNER_FRAMES[app.tick as usize % SPINNER_FRAMES.len()]
    );

    let is_browsing = matches!(app.mode, AppMode::Done { .. } | AppMode::Error(_));

    let mut step_lines: Vec<Line> = Vec::new();
    for (i, record) in app.steps.iter().enumerate() {
        let step = &record.step;
        let status = &record.status;
        let is_interactive = matches!(step, PipelineStep::Select | PipelineStep::QualityCheck);
        let (icon, mut style): (&str, Style) = match status {
            StepStatus::Pending => ("  ", Style::default().fg(THEME.dimmed)),
            StepStatus::Running(_) if is_interactive => ("▸ ", Style::default().fg(THEME.info)),
            StepStatus::Running(_) => (&spinner_frame, Style::default().fg(THEME.info)),
            StepStatus::Done(_) => ("✓ ", Style::default().fg(THEME.success)),
            StepStatus::Skipped => ("- ", Style::default().fg(THEME.dimmed)),
            StepStatus::Error(_) => ("✗ ", Style::default().fg(THEME.danger)),
        };

        // Highlight selected step in browse mode
        let is_selected = is_browsing && app.browse_step == Some(i);
        if is_selected {
            style = style.bg(THEME.highlight_bg);
        }

        let detail = match status {
            StepStatus::Running(Some(d)) | StepStatus::Done(Some(d)) => Some(d.as_str()),
            StepStatus::Error(_) => None,
            _ => None,
        };

        // Show detail inline if it fits, otherwise on a second line
        let sidebar_inner = 28; // 30 - border - padding
        if let Some(d) = detail {
            let inline_len = icon.len() + step.label().len() + 2 + d.len();
            if inline_len <= sidebar_inner {
                step_lines.push(Line::from(vec![
                    Span::styled(icon, style),
                    Span::styled(step.label(), style),
                    Span::styled(
                        format!("  {d}"),
                        Style::default().fg(THEME.dimmed).bg(if is_selected {
                            THEME.highlight_bg
                        } else {
                            Color::Reset
                        }),
                    ),
                ]));
            } else {
                step_lines.push(Line::from(vec![
                    Span::styled(icon, style),
                    Span::styled(step.label(), style),
                ]));
                step_lines.push(Line::from(Span::styled(
                    format!("    {d}"),
                    Style::default().fg(THEME.dimmed),
                )));
            }
        } else {
            step_lines.push(Line::from(vec![
                Span::styled(icon, style),
                Span::styled(step.label(), style),
            ]));
        }
    }

    let steps_height = step_lines.len() as u16;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(info_height),
            Constraint::Length(steps_height),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(inner);

    // Session info
    if let Some(info) = &app.session_info {
        let mut lines = vec![
            Line::from(vec![
                Span::styled("Deck  ", Style::default().fg(THEME.dimmed)),
                Span::raw(&info.deck),
            ]),
            Line::from(vec![
                Span::styled("Note  ", Style::default().fg(THEME.dimmed)),
                Span::raw(&info.note_type),
            ]),
            Line::from(vec![
                Span::styled("Model ", Style::default().fg(THEME.dimmed)),
                Span::raw(&info.model),
            ]),
        ];
        if let Some(term) = &app.last_term
            && has_term
        {
            let label = if let Some((current, total)) = app.batch_progress {
                format!("{current}/{total} ")
            } else {
                String::new()
            };
            lines.push(Line::from(vec![
                Span::styled("Term  ", Style::default().fg(THEME.dimmed)),
                Span::styled(label, Style::default().fg(THEME.info)),
                Span::styled(
                    term.clone(),
                    Style::default().fg(THEME.text).add_modifier(Modifier::BOLD),
                ),
            ]));
        }
        frame.render_widget(Paragraph::new(lines), chunks[0]);
    }

    // Pipeline steps
    frame.render_widget(Paragraph::new(step_lines), chunks[1]);

    // Cost
    let total = app.session_cost + app.run_cost;
    if total > 0.0 {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                pricing::format_cost(total),
                Style::default().fg(THEME.dimmed),
            ))),
            chunks[3],
        );
    }
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let mut s: Vec<Span<'static>> = vec![Span::raw(" ")];

    match &app.mode {
        AppMode::Input(_) => {
            s.extend(footer_cmd("Enter", "Generate"));
            s.push(footer_pipe());
            s.extend(footer_cmd("Tab", "Queue term"));
            s.push(footer_pipe());
            s.extend(footer_cmd("Ctrl+O", "Model"));
            s.push(footer_pipe());
            s.extend(footer_cmd("↑↓", "History"));
            s.push(footer_pipe());
            s.extend(footer_cmd("Ctrl+C", "Quit"));
        }
        AppMode::Running => {
            if let Some((current, total)) = app.batch_progress {
                let spinner = SPINNER_FRAMES[app.tick as usize % SPINNER_FRAMES.len()];
                s.push(Span::styled(
                    format!("{spinner} Batch {current}/{total}"),
                    Style::default().fg(THEME.info),
                ));
                s.push(footer_pipe());
            }
            s.extend(footer_cmd("Esc", "Cancel"));
            s.push(footer_pipe());
            s.extend(footer_cmd("q", "Quit"));
            s.push(footer_pipe());
            s.extend(footer_cmd("?", "Help"));
        }
        AppMode::Selecting(state) => {
            let n = state.selected.len();
            let focused_is_dup = state
                .cards
                .get(state.cursor)
                .map(|c| c.is_duplicate)
                .unwrap_or(false);
            s.extend(footer_cmd("Space", "Toggle"));
            s.push(footer_pipe());
            if focused_is_dup {
                s.extend(footer_cmd("f", "Force"));
                s.push(footer_pipe());
            }
            s.extend(footer_cmd("a", "All"));
            s.push(footer_pipe());
            s.extend(footer_cmd("n", "None"));
            s.push(footer_pipe());
            s.extend(footer_cmd("c", "Copy"));
            s.push(footer_pipe());
            s.extend(footer_cmd("d", "Remove"));
            s.push(footer_pipe());
            s.extend(footer_cmd("e", "Edit"));
            s.push(footer_pipe());
            if app
                .session_info
                .as_ref()
                .map(|info| info.tts_configured)
                .unwrap_or(false)
                && app.player.is_some()
            {
                s.extend(footer_cmd("p", "Preview"));
                s.push(footer_pipe());
            }
            if state.refresh_in_flight || state.regen_in_flight.is_some() {
                let spinner = SPINNER_FRAMES[app.tick as usize % SPINNER_FRAMES.len()];
                let loading_text = if let Some((current, total)) = app.batch_progress {
                    format!("{spinner} Batch {current}/{total}...")
                } else {
                    format!("{spinner} Loading...")
                };
                s.push(Span::styled(loading_text, Style::default().fg(THEME.info)));
            } else if state.term_input.is_some() || state.feedback_input.is_some() {
                s.extend(footer_cmd("Enter", "Submit"));
                s.push(footer_pipe());
                s.extend(footer_cmd("Esc", "Cancel"));
            } else {
                s.extend(footer_cmd("r", "More"));
                s.push(footer_pipe());
                s.extend(footer_cmd("t", "New term"));
                s.push(footer_pipe());
                s.extend(footer_cmd("R", "Regen"));
            }
            s.push(footer_pipe());
            s.extend(footer_cmd("Ctrl+O", "Model"));
            s.push(footer_pipe());
            s.extend(footer_cmd("Enter", "Confirm"));
            s.push(footer_pipe());
            s.extend(footer_cmd("Esc", "Back"));
            s.push(footer_pipe());
            s.extend(footer_cmd("q", "Quit"));
            s.push(footer_pipe());
            s.extend(footer_cmd("?", "Help"));
            s.push(Span::styled(
                format!("  ({n} selected)"),
                Style::default().fg(THEME.dimmed),
            ));
        }
        AppMode::Reviewing(state) => {
            let cur = (state.cursor + 1).min(state.flagged.len());
            let total = state.flagged.len();
            s.push(Span::styled(
                format!("Flagged {cur}/{total}"),
                Style::default().fg(THEME.warning),
            ));
            s.push(footer_pipe());
            s.extend(footer_cmd("k", "Keep"));
            s.push(footer_pipe());
            s.extend(footer_cmd("d", "Discard"));
            s.push(footer_pipe());
            s.extend(footer_cmd("u", "Back"));
            s.push(footer_pipe());
            s.extend(footer_cmd("a", "Keep all"));
            s.push(footer_pipe());
            s.extend(footer_cmd("x", "Discard all"));
            s.push(footer_pipe());
            s.extend(footer_cmd("q", "Quit"));
            s.push(footer_pipe());
            s.extend(footer_cmd("?", "Help"));
        }
        AppMode::Done {
            note_ids, cards, ..
        } => {
            s.extend(footer_cmd("j/k", "Steps"));
            s.push(footer_pipe());
            if !app.is_fatal {
                s.extend(footer_cmd("n", "New term"));
                if app.last_term.is_some() {
                    s.push(footer_pipe());
                    s.extend(footer_cmd("r", "Retry"));
                }
                s.push(footer_pipe());
                s.extend(footer_cmd("Ctrl+O", "Model"));
                if !cards.is_empty() {
                    s.push(footer_pipe());
                    s.extend(footer_cmd("c", "Copy"));
                }
                if !note_ids.is_empty() {
                    s.push(footer_pipe());
                    s.extend(footer_cmd("d", "Delete"));
                }
                s.push(footer_pipe());
            }
            s.extend(footer_cmd("q", "Quit"));
            s.push(footer_pipe());
            s.extend(footer_cmd("?", "Help"));
        }
        AppMode::Error(_) => {
            s.extend(footer_cmd("j/k", "Steps"));
            s.push(footer_pipe());
            if !app.is_fatal {
                s.extend(footer_cmd("n", "New term"));
                if app.last_term.is_some() {
                    s.push(footer_pipe());
                    s.extend(footer_cmd("r", "Retry"));
                }
                s.push(footer_pipe());
                s.extend(footer_cmd("Ctrl+O", "Model"));
                s.push(footer_pipe());
            }
            s.extend(footer_cmd("q", "Quit"));
            s.push(footer_pipe());
            s.extend(footer_cmd("?", "Help"));
        }
    }

    frame.render_widget(Paragraph::new(Line::from(s)), area);
}

fn draw_input(
    frame: &mut Frame,
    input: &LineInput,
    history_pos: Option<(usize, usize)>,
    batch_queued: usize,
    area: Rect,
    show_cursor: bool,
) {
    // Center the input box in the main area
    let max_width = 50u16.min(area.width.saturating_sub(4));
    let h_pad = area.width.saturating_sub(max_width) / 2;

    let h_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(h_pad),
            Constraint::Length(max_width),
            Constraint::Min(0),
        ])
        .split(area);

    let col = h_chunks[1];
    let input_height: u16 = 3;
    let v_pad = col.height.saturating_sub(input_height) / 2;

    let v_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(v_pad),
            Constraint::Length(input_height),
            Constraint::Min(0),
        ])
        .split(col);

    let input_block_area = Rect {
        height: 3,
        ..v_chunks[1]
    };
    let inner_width = input_block_area.width.saturating_sub(2).max(1) as usize;
    let scroll = input.visual_scroll(inner_width);

    let title = if batch_queued > 0 {
        format!(" Enter term ({} queued) ", batch_queued)
    } else {
        " Enter term ".to_string()
    };

    let mut block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(THEME.info));

    if let Some((pos, total)) = history_pos {
        let indicator = format!(" {pos}/{total} ");
        block = block.title(
            Line::from(Span::styled(indicator, Style::default().fg(THEME.dimmed))).right_aligned(),
        );
    }

    let para = Paragraph::new(input.value())
        .block(block)
        .scroll((0, scroll as u16));
    frame.render_widget(para, input_block_area);

    if show_cursor {
        frame.set_cursor_position((
            input_block_area.x + 1 + (input.visual_cursor().saturating_sub(scroll)) as u16,
            input_block_area.y + 1,
        ));
    }
}

fn draw_running(frame: &mut Frame, app: &App, area: Rect) {
    // Steps are in the sidebar; main area is just the log
    draw_log_panel(frame, &app.logs, app.log_scroll, area);
}

fn draw_done(
    frame: &mut Frame,
    app: &App,
    msg: &str,
    cards: &[ValidatedCard],
    failed: bool,
    area: Rect,
) {
    let (header_text, header_color, body_color) = if failed {
        ("✗ Failed", THEME.danger, THEME.danger)
    } else {
        ("✓ Done", THEME.success, THEME.text)
    };
    let mut summary_lines = vec![
        Line::from(Span::styled(
            header_text,
            Style::default()
                .fg(header_color)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(msg, Style::default().fg(body_color))),
    ];

    if app.run_cost > 0.0 {
        summary_lines.push(Line::from(""));
        summary_lines.push(Line::from(format!(
            "Tokens: {} in / {} out  |  Cost: {}",
            app.run_input_tokens,
            app.run_output_tokens,
            pricing::format_cost(app.run_cost)
        )));
        if app.session_cost > 0.0 {
            summary_lines.push(Line::from(format!(
                "Session total: {}",
                pricing::format_cost(app.session_cost + app.run_cost)
            )));
        }
    }

    if cards.is_empty() {
        let para = Paragraph::new(Text::from(summary_lines)).wrap(Wrap { trim: false });
        frame.render_widget(para, area);
        return;
    }

    let summary_height = summary_lines.len() as u16 + 1; // +1 for spacing
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(summary_height), Constraint::Min(0)])
        .split(area);

    let para = Paragraph::new(Text::from(summary_lines)).wrap(Wrap { trim: false });
    frame.render_widget(para, chunks[0]);

    // Show final cards
    let mut card_lines: Vec<Line> = Vec::new();
    for (i, card) in cards.iter().enumerate() {
        if i > 0 {
            card_lines.push(Line::from(Span::styled(
                "─".repeat(40),
                Style::default().fg(THEME.border),
            )));
        }
        for (name, value) in &card.raw_anki_fields {
            card_lines.push(Line::from(Span::styled(
                name.clone(),
                Style::default().fg(THEME.info).add_modifier(Modifier::BOLD),
            )));
            card_lines.extend(super::selector::markdown_to_lines(value, "  "));
            card_lines.push(Line::from(""));
        }
    }

    let card_block = Block::default()
        .borders(Borders::ALL.difference(Borders::LEFT))
        .title(format!(" Cards ({}) ", cards.len()))
        .border_style(Style::default().fg(THEME.border));
    let card_para = Paragraph::new(Text::from(card_lines))
        .block(card_block)
        .wrap(Wrap { trim: false })
        .scroll((app.browse_scroll, 0));
    frame.render_widget(card_para, chunks[1]);
}

fn draw_error(frame: &mut Frame, msg: &str, area: Rect) {
    let lines = vec![
        Line::from(Span::styled(
            "✗ Error",
            Style::default()
                .fg(THEME.danger)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(msg, Style::default().fg(THEME.danger))),
    ];

    let para = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
    frame.render_widget(para, area);
}

// ---------------------------------------------------------------------------
// Main event loop
// ---------------------------------------------------------------------------

enum ExitReason {
    UserQuit,
    NaturalExit,
    SwitchPrompt,
}

fn run_app(
    mut terminal: DefaultTerminal,
    initial_term: Option<String>,
    glyphs: Glyphs,
    backend_rx: mpsc::Receiver<BackendEvent>,
    worker_tx: mpsc::SyncSender<WorkerCommand>,
) -> anyhow::Result<ExitReason> {
    let mut app = App::new(initial_term, glyphs, backend_rx, worker_tx);

    loop {
        app.tick = app.tick.wrapping_add(1);
        terminal.draw(|f| draw(f, &app))?;

        // Drain all pending backend events
        loop {
            match app.backend_rx.try_recv() {
                Ok(ev) => app.handle_backend_event(ev),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    if !matches!(app.mode, AppMode::Done { .. } | AppMode::Error(_)) {
                        app.mode = AppMode::Error("Worker thread exited unexpectedly".to_string());
                    }
                    break;
                }
            }
        }

        if app.should_quit {
            break;
        }

        // Poll for terminal input (50 ms timeout so we don't block backend events)
        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(key) => app.handle_key(key),
                Event::Paste(text) => app.handle_paste_input(text),
                _ => {}
            }
        }

        // Handle pending editor launch (needs terminal access)
        if let Some(card_index) = app.pending_edit.take() {
            edit_card_in_editor(&mut terminal, &mut app, card_index);
        }

        if app.should_quit {
            break;
        }
    }

    if app.switch_prompt {
        Ok(ExitReason::SwitchPrompt)
    } else if app.user_quit {
        Ok(ExitReason::UserQuit)
    } else {
        Ok(ExitReason::NaturalExit)
    }
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

pub fn run_tui(mut args: GenerateArgs) -> anyhow::Result<()> {
    use crate::workspace::resolver::{ResolvedPrompt, resolve_prompt, save_last_prompt};

    let mut force_picker = false;
    loop {
        // Resolve prompt before entering the TUI. If multiple prompts are
        // available and none was specified, show an interactive picker.
        if args.prompt.is_none() {
            match resolve_prompt(None, force_picker)? {
                ResolvedPrompt::Resolved(path) => {
                    save_last_prompt(&path);
                    args.prompt = Some(path);
                }
                ResolvedPrompt::ShowPicker(prompts) => {
                    let terminal = ratatui::init();
                    let glyphs = Glyphs::from_config();
                    let result = run_prompt_picker(terminal, &prompts, &glyphs);
                    ratatui::restore();
                    match result {
                        Some(path) => {
                            save_last_prompt(&path);
                            args.prompt = Some(path);
                        }
                        None => return Ok(()), // user cancelled
                    }
                }
            }
        }

        let initial_term = args.term.take(); // only use CLI term on first iteration

        let (tx_events, rx_events) = mpsc::channel::<BackendEvent>();
        let (tx_cmd, rx_cmd) = mpsc::sync_channel::<WorkerCommand>(10);

        let pipeline_args = GenerateArgs {
            prompt: args.prompt.clone(),
            term: initial_term.clone(),
            count: args.count,
            model: args.model.clone(),
            api_base_url: args.api_base_url.clone(),
            api_key: args.api_key.clone(),
            dry_run: args.dry_run,
            retries: args.retries,
            max_tokens: args.max_tokens,
            temperature: args.temperature,
            output: args.output.clone(),
            copy: args.copy,
            log: args.log.clone(),
            very_verbose: args.very_verbose,
        };

        let worker_handle = std::thread::spawn(move || {
            super::command_generate::run_pipeline(pipeline_args, tx_events, rx_cmd)
        });

        let glyphs = Glyphs::from_config();
        let terminal = ratatui::init();
        crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste).ok();
        let exit = run_app(terminal, initial_term, glyphs, rx_events, tx_cmd)
            .unwrap_or(ExitReason::UserQuit);
        crossterm::execute!(std::io::stdout(), crossterm::event::DisableBracketedPaste).ok();
        ratatui::restore();

        match exit {
            ExitReason::SwitchPrompt => {
                args.prompt = None;
                force_picker = true;
                continue;
            }
            ExitReason::UserQuit => {
                std::process::exit(0);
            }
            ExitReason::NaturalExit => {
                return worker_handle
                    .join()
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("Worker thread panicked")));
            }
        }
    }
}

use prompt_picker::run_prompt_picker;

/// Deserialize the user's edited YAML back into an Anki-field-name
/// keyed map. Extracted so the post-`$EDITOR` parse + first-field
/// lookup is unit-testable without spawning an editor.
fn parse_edited_anki_fields(
    yaml: &str,
) -> Result<indexmap::IndexMap<String, String>, serde_yaml::Error> {
    serde_yaml::from_str(yaml)
}

/// True when *any* card in the selection has a TTS preview in flight.
/// Used by the Enter/Esc guards to block terminal actions that would
/// otherwise race the worker's FIFO action queue behind an in-flight
/// `PreviewTts` — issue #9. The optimistic `Synthesizing` state set
/// on the `p` keypress is what makes this guard fire immediately,
/// before the worker's own `TtsState::Synthesizing` reply
/// round-trips.
///
/// The check is selection-global, not focused-row-local: the worker
/// command channel is a shared FIFO, so once *any* card is in
/// `Synthesizing`, any `Selection` or `Cancel` the user sends queues
/// behind that `PreviewTts` and re-opens the race — moving the
/// cursor to a different card doesn't help.
fn any_card_synthesizing(state: &SelectionState) -> bool {
    state
        .tts_states
        .values()
        .any(|s| matches!(s, TtsUiState::Synthesizing))
}

#[cfg(test)]
mod tests {
    use super::{SelectionState, TtsUiState, any_card_synthesizing, parse_edited_anki_fields};
    use crate::generate::cards::{ValidatedCard, next_card_id};
    use indexmap::IndexMap;

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

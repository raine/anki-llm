use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, List, ListItem, ListState, Paragraph, Row, Table, Wrap,
};
use ratatui::{DefaultTerminal, Frame};

use super::line_input::LineInput;

use crate::anki::client::AnkiClient;
use crate::cli::GenerateArgs;
use crate::config::store::read_config;
use crate::llm::pricing;

use super::cards::ValidatedCard;
use super::process::FlaggedCard;

// ---------------------------------------------------------------------------
// Events / responses
// ---------------------------------------------------------------------------

pub struct SessionInfo {
    pub deck: String,
    pub note_type: String,
    pub model: String,
    pub available_models: Vec<String>,
}

pub enum BackendEvent {
    SessionReady(SessionInfo),
    Log(String),
    StepUpdate {
        step: PipelineStep,
        status: StepStatus,
    },
    RequestSelection(Vec<ValidatedCard>),
    AppendCards(Vec<ValidatedCard>), // refresh: new unique cards to append
    RequestReview(Vec<FlaggedCard>),
    CostUpdate {
        input_tokens: u64,
        output_tokens: u64,
        cost: f64,
    },
    RunDone {
        message: String,
        cards: Vec<ValidatedCard>,
        /// Anki note IDs of imported cards (empty for exports/dry runs).
        note_ids: Vec<i64>,
    },
    RunError(String),         // single run failed (can retry with new term)
    ModelChangeError(String), // model switch failed
    Fatal(String),            // session-level error (must exit)
}

pub enum WorkerCommand {
    Start(String), // term to generate cards for
    Refresh,       // generate more cards for the same term
    Selection(Vec<usize>),
    Review(Vec<bool>), // true = keep, false = discard
    SetModel(String),  // change model between runs
    Cancel,            // abandon current run, go back to input
    Quit,
}

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

#[derive(Clone, PartialEq)]
pub enum StepStatus {
    Pending,
    Running(Option<String>),
    Done(Option<String>),
    Skipped,
    Error(String),
}

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
    },
    Error(String),
}

struct SelectionState {
    cards: Vec<ValidatedCard>,
    cursor: usize,
    selected: BTreeSet<usize>,
    list_state: ListState,
    detail_scroll: u16,
    /// True while a refresh (load more) request is in flight.
    refresh_in_flight: bool,
}

impl SelectionState {
    fn new(cards: Vec<ValidatedCard>) -> Self {
        let mut list_state = ListState::default();
        list_state.select(Some(0));
        let selected = BTreeSet::new();
        Self {
            cards,
            cursor: 0,
            selected,
            list_state,
            detail_scroll: 0,
            refresh_in_flight: false,
        }
    }

    fn move_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.list_state.select(Some(self.cursor));
            self.detail_scroll = 0;
        }
    }

    fn move_down(&mut self) {
        if self.cursor + 1 < self.cards.len() {
            self.cursor += 1;
            self.list_state.select(Some(self.cursor));
            self.detail_scroll = 0;
        }
    }

    fn toggle_current(&mut self) {
        if self
            .cards
            .get(self.cursor)
            .map(|c| c.is_duplicate)
            .unwrap_or(false)
        {
            return; // Duplicates cannot be selected
        }
        if self.selected.contains(&self.cursor) {
            self.selected.remove(&self.cursor);
        } else {
            self.selected.insert(self.cursor);
        }
    }

    fn select_all(&mut self) {
        for (i, c) in self.cards.iter().enumerate() {
            if !c.is_duplicate {
                self.selected.insert(i);
            }
        }
    }

    fn select_none(&mut self) {
        self.selected.clear();
    }
}

struct ReviewState {
    flagged: Vec<FlaggedCard>,
    cursor: usize,        // which flagged card we're reviewing
    decisions: Vec<bool>, // true = keep, false = discard
    detail_scroll: u16,
}

impl ReviewState {
    fn new(flagged: Vec<FlaggedCard>) -> Self {
        let len = flagged.len();
        Self {
            flagged,
            cursor: 0,
            decisions: vec![false; len],
            detail_scroll: 0,
        }
    }

    fn current(&self) -> Option<&FlaggedCard> {
        self.flagged.get(self.cursor)
    }

    fn keep_current(&mut self) {
        if self.cursor < self.decisions.len() {
            self.decisions[self.cursor] = true;
        }
        self.advance();
    }

    fn discard_current(&mut self) {
        if self.cursor < self.decisions.len() {
            self.decisions[self.cursor] = false;
        }
        self.advance();
    }

    fn keep_all(&mut self) {
        for d in &mut self.decisions {
            *d = true;
        }
        self.cursor = self.flagged.len(); // done
    }

    fn discard_all(&mut self) {
        for d in &mut self.decisions {
            *d = false;
        }
        self.cursor = self.flagged.len(); // done
    }

    fn advance(&mut self) {
        self.cursor += 1;
        self.detail_scroll = 0;
    }

    fn move_back(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.detail_scroll = 0;
        }
    }

    fn is_done(&self) -> bool {
        self.cursor >= self.flagged.len()
    }
}

struct ModelPickerState {
    models: Vec<String>,
    cursor: usize,
    list_state: ListState,
}

impl ModelPickerState {
    fn new(models: Vec<String>, current_model: Option<&str>) -> Self {
        let cursor = current_model
            .and_then(|m| models.iter().position(|s| s == m))
            .unwrap_or(0);
        let mut list_state = ListState::default();
        list_state.select(Some(cursor));
        Self {
            models,
            cursor,
            list_state,
        }
    }

    fn move_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.list_state.select(Some(self.cursor));
        }
    }

    fn move_down(&mut self) {
        if self.cursor + 1 < self.models.len() {
            self.cursor += 1;
            self.list_state.select(Some(self.cursor));
        }
    }

    fn selected(&self) -> Option<&str> {
        self.models.get(self.cursor).map(|s| s.as_str())
    }
}

const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

// ---------------------------------------------------------------------------
// Glyph sets (Nerd Font vs plain fallback)
// ---------------------------------------------------------------------------

struct Glyphs {
    checkbox_checked: &'static str,
    checkbox_unchecked: &'static str,
}

impl Glyphs {
    fn nerd() -> Self {
        Self {
            checkbox_checked: "\u{f4a7} ",
            checkbox_unchecked: "\u{f0131} ",
        }
    }

    fn plain() -> Self {
        Self {
            checkbox_checked: "[x] ",
            checkbox_unchecked: "[ ] ",
        }
    }

    fn from_config() -> Self {
        let nerd_font = read_config().ok().and_then(|c| c.nerd_font).unwrap_or(true);
        if nerd_font {
            Self::nerd()
        } else {
            Self::plain()
        }
    }
}

// ---------------------------------------------------------------------------
// Theme palette (matches workmux default dark)
// ---------------------------------------------------------------------------

struct Palette {
    dimmed: Color,
    text: Color,
    border: Color,
    info: Color,
    success: Color,
    warning: Color,
    danger: Color,
    highlight_bg: Color,
    highlight_fg: Color,
    help_border: Color,
    help_muted: Color,
    header: Color,
}

const THEME: Palette = Palette {
    dimmed: Color::Rgb(108, 112, 134),
    text: Color::Rgb(205, 214, 244),
    border: Color::Rgb(58, 74, 94),
    info: Color::Rgb(120, 225, 213),
    success: Color::Rgb(166, 218, 149),
    warning: Color::Rgb(249, 226, 175),
    danger: Color::Rgb(237, 135, 150),
    highlight_bg: Color::Rgb(40, 48, 62),
    highlight_fg: Color::Rgb(244, 248, 255),
    help_border: Color::Rgb(81, 104, 130),
    help_muted: Color::Rgb(112, 126, 144),
    header: Color::Rgb(180, 200, 220),
};

/// Build a footer span pair: key in dimmed, label in bold text.
fn footer_cmd(key: &str, label: &str) -> Vec<Span<'static>> {
    vec![
        Span::styled(key.to_string(), Style::default().fg(THEME.dimmed)),
        Span::styled(
            format!(" {label}"),
            Style::default().fg(THEME.text).add_modifier(Modifier::BOLD),
        ),
    ]
}

fn footer_pipe() -> Span<'static> {
    Span::styled(" \u{2502} ", Style::default().fg(THEME.border))
}

// ---------------------------------------------------------------------------
// Input history
// ---------------------------------------------------------------------------

const HISTORY_MAX: usize = 100;

struct InputHistory {
    entries: Vec<String>,
    /// Index into entries (0 = most recent). `None` = not browsing history.
    cursor: Option<usize>,
    /// Text the user was typing before they started browsing history.
    stashed: String,
}

impl InputHistory {
    fn load() -> Self {
        let mut entries = Self::path()
            .and_then(|p| fs::read_to_string(p).ok())
            .map(|s| s.lines().rev().map(String::from).collect::<Vec<_>>())
            .unwrap_or_default();
        entries.truncate(HISTORY_MAX);
        InputHistory {
            entries,
            cursor: None,
            stashed: String::new(),
        }
    }

    fn path() -> Option<PathBuf> {
        home::home_dir().map(|h| {
            h.join(".local")
                .join("state")
                .join("anki-llm")
                .join("history")
        })
    }

    fn push(&mut self, term: &str) {
        // Don't add duplicate of most recent entry
        if self.entries.first().is_some_and(|e| e == term) {
            return;
        }
        self.entries.insert(0, term.to_string());
        self.entries.truncate(HISTORY_MAX);
        self.save();
    }

    fn save(&self) {
        let Some(path) = Self::path() else { return };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).ok();
        }
        let tmp = path.with_extension("tmp");
        let ok = (|| -> std::io::Result<()> {
            let mut f = std::io::BufWriter::new(fs::File::create(&tmp)?);
            for entry in self.entries.iter().rev() {
                writeln!(f, "{entry}")?;
            }
            f.flush()?;
            Ok(())
        })();
        if ok.is_ok() {
            let _ = fs::rename(&tmp, &path);
        } else {
            let _ = fs::remove_file(&tmp);
        }
    }

    /// Move up (older). Returns the history entry to show.
    fn up(&mut self, current_text: &str) -> Option<&str> {
        let next = match self.cursor {
            None => {
                self.stashed = current_text.to_string();
                0
            }
            Some(i) => i + 1,
        };
        if next < self.entries.len() {
            self.cursor = Some(next);
            Some(&self.entries[next])
        } else {
            self.cursor
                .and_then(|i| self.entries.get(i).map(|s| s.as_str()))
        }
    }

    /// Move down (newer). Returns the history entry, or `None` if not browsing.
    fn down(&mut self) -> Option<&str> {
        match self.cursor {
            None => None,
            Some(0) => {
                self.cursor = None;
                Some(&self.stashed)
            }
            Some(i) => {
                let next = i - 1;
                self.cursor = Some(next);
                Some(&self.entries[next])
            }
        }
    }

    fn reset_browse(&mut self) {
        self.cursor = None;
        self.stashed.clear();
    }

    /// Returns `(1-based position, total)` when browsing history, or `None`.
    fn browse_position(&self) -> Option<(usize, usize)> {
        self.cursor.map(|i| (i + 1, self.entries.len()))
    }
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
    /// True after a Fatal error — worker is dead, no new runs possible.
    is_fatal: bool,
    glyphs: Glyphs,
    history: InputHistory,
    toast: Option<Toast>,
    /// In Done/Error mode: selected step index for log browsing, None = summary.
    browse_step: Option<usize>,
    browse_scroll: u16,
    backend_rx: mpsc::Receiver<BackendEvent>,
    worker_tx: mpsc::SyncSender<WorkerCommand>,
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
            is_fatal: false,
            glyphs,
            history: InputHistory::load(),
            toast: None,
            browse_step: None,
            browse_scroll: 0,
            backend_rx,
            worker_tx,
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
                self.mode = AppMode::Selecting(SelectionState::new(cards));
            }
            BackendEvent::AppendCards(new_cards) => {
                if let AppMode::Selecting(ref mut state) = self.mode {
                    state.cards.extend(new_cards);
                    state.refresh_in_flight = false;
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
            } => {
                self.mode = AppMode::Done {
                    message,
                    cards,
                    note_ids,
                };
                self.current_step_idx = None;
            }
            BackendEvent::RunError(msg) => {
                self.mode = AppMode::Error(msg);
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
                KeyCode::Up | KeyCode::Char('k') => picker.move_up(),
                KeyCode::Down | KeyCode::Char('j') => picker.move_down(),
                KeyCode::Enter => {
                    if let Some(model) = picker.selected().map(|s| s.to_string()) {
                        let changed = self
                            .session_info
                            .as_ref()
                            .map(|s| s.model != model)
                            .unwrap_or(true);
                        if changed {
                            // In Selecting mode: cancel current run, switch model,
                            // and re-run with the same term.
                            if matches!(self.mode, AppMode::Selecting(_)) {
                                self.worker_tx.send(WorkerCommand::Cancel).ok();
                                self.pending_cancels += 1;
                                self.reset_for_new_run();
                                self.worker_tx.send(WorkerCommand::SetModel(model)).ok();
                                if let Some(term) = self.last_term.clone() {
                                    self.mode = AppMode::Running;
                                    self.worker_tx.send(WorkerCommand::Start(term)).ok();
                                } else {
                                    self.mode = AppMode::Input(LineInput::default());
                                }
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

        // Toggle help overlay from any mode
        if key.code == KeyCode::Char('?') && !matches!(self.mode, AppMode::Input(_)) {
            self.show_help = true;
            return;
        }

        match &mut self.mode {
            AppMode::Input(_) => self.handle_key_input(key),
            AppMode::Running => match key.code {
                KeyCode::Esc => {
                    // Cancel current run and go back to term input.
                    // Send Cancel so if the worker reaches a recv() (e.g.
                    // RequestSelection) it unblocks. Events from the
                    // abandoned run are discarded until RunDone/RunError.
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
                if !input.value().is_empty() {
                    input.reset();
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
            KeyCode::Enter => {
                let term = input.value().trim().to_string();
                if !term.is_empty() {
                    self.history.push(&term);
                    self.history.reset_browse();
                    self.last_term = Some(term.clone());
                    self.mode = AppMode::Running;
                    self.worker_tx.send(WorkerCommand::Start(term)).ok();
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
        let AppMode::Input(ref mut input) = self.mode else {
            return;
        };
        if input.handle_event(&Event::Paste(text)) {
            self.history.reset_browse();
        }
    }

    fn handle_key_selection(&mut self, key: crossterm::event::KeyEvent) {
        let AppMode::Selecting(ref mut state) = self.mode else {
            return;
        };

        match key.code {
            KeyCode::Up | KeyCode::Char('k') => state.move_up(),
            KeyCode::Down | KeyCode::Char('j') => state.move_down(),
            KeyCode::Char(' ') => state.toggle_current(),
            KeyCode::Char('a') => state.select_all(),
            KeyCode::Char('n') => state.select_none(),
            KeyCode::Char('r') if !state.refresh_in_flight => {
                state.refresh_in_flight = true;
                self.worker_tx.send(WorkerCommand::Refresh).ok();
            }
            KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.open_model_picker();
            }
            KeyCode::Esc => {
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
                let AppMode::Selecting(state) = std::mem::replace(&mut self.mode, AppMode::Running)
                else {
                    return;
                };
                let indices: Vec<usize> = state.selected.into_iter().collect();
                self.worker_tx.send(WorkerCommand::Selection(indices)).ok();
            }
            KeyCode::Char('c') => {
                if let Some(card) = state.cards.get(state.cursor) {
                    let text = card
                        .raw_anki_fields
                        .iter()
                        .map(|(name, value)| {
                            let plain = super::selector::strip_html_tags(value);
                            format!("{name}\n{plain}")
                        })
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    if let Ok(mut cb) = arboard::Clipboard::new() {
                        cb.set_text(text).ok();
                    }
                    self.toast = Some(Toast {
                        message: "Copied!".into(),
                        tick: self.tick,
                    });
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
            main_area,
            app.model_picker.is_none(),
        ),
        AppMode::Running => draw_running(frame, app, main_area),
        AppMode::Selecting(state) => draw_selecting(frame, state, &app.glyphs, main_area),
        AppMode::Reviewing(state) => draw_reviewing(frame, state, main_area),
        AppMode::Done { message, cards, .. } => {
            if let Some(step_idx) = app.browse_step {
                draw_step_logs(frame, app, step_idx, main_area);
            } else {
                draw_done(frame, app, message, cards, main_area);
            }
        }
        AppMode::Error(msg) => {
            if let Some(step_idx) = app.browse_step {
                draw_step_logs(frame, app, step_idx, main_area);
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
        AppMode::Selecting(_) => vec![
            ("Space", "Toggle"),
            ("a", "All"),
            ("n", "None"),
            ("c", "Copy"),
            ("r", "More"),
            ("Ctrl+O", "Model"),
            ("Enter", "Confirm"),
            ("Esc", "Back"),
            ("q", "Quit"),
            ("PgUp/PgDn", "Scroll"),
        ],
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

fn draw_model_picker(frame: &mut Frame, picker: &ModelPickerState) {
    let row_count = picker.models.len() as u16;
    let height = (row_count + 2).min(20); // borders
    let width: u16 = 48;

    let area = frame.area();
    let rect = Rect::new(
        area.width.saturating_sub(width) / 2,
        area.height.saturating_sub(height) / 2,
        width.min(area.width),
        height.min(area.height),
    );

    let block = Block::bordered()
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(THEME.help_border))
        .title(Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled(
                "Model",
                Style::default()
                    .fg(THEME.header)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ", Style::default()),
        ]))
        .title_bottom(Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled("Enter", Style::default().fg(THEME.dimmed)),
            Span::styled(" select ", Style::default().fg(THEME.help_muted)),
            Span::styled("Esc", Style::default().fg(THEME.dimmed)),
            Span::styled(" cancel ", Style::default().fg(THEME.help_muted)),
        ]));

    // Inner width: total width - 2 borders - 2 highlight symbol
    let inner_w = width.saturating_sub(4) as usize;

    let items: Vec<ListItem> = picker
        .models
        .iter()
        .map(|m| {
            let price = pricing::model_pricing(m)
                .map(|p| format_model_price(p.input_cost_per_million, p.output_cost_per_million))
                .unwrap_or_default();
            let pad = inner_w.saturating_sub(m.len() + price.len());
            ListItem::new(Line::from(vec![
                Span::styled(m.as_str(), Style::default().fg(THEME.text)),
                Span::raw(" ".repeat(pad)),
                Span::styled(price, Style::default().fg(THEME.dimmed)),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .fg(THEME.highlight_fg)
                .bg(THEME.highlight_bg)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");

    let mut list_state = picker.list_state;

    frame.render_widget(Clear, rect);
    frame.render_stateful_widget(list, rect, &mut list_state);
}

/// Format pricing as compact "$in/$out" per million tokens.
fn format_model_price(input: f64, output: f64) -> String {
    fn fmt(v: f64) -> String {
        if v == (v as u64) as f64 {
            format!("${}", v as u64)
        } else if v * 10.0 == (v * 10.0).round() {
            format!("${:.1}", v)
        } else {
            format!("${:.2}", v)
        }
    }
    format!("{}/{}", fmt(input), fmt(output))
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
            lines.push(Line::from(vec![
                Span::styled("Term  ", Style::default().fg(THEME.dimmed)),
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
            s.extend(footer_cmd("Ctrl+O", "Model"));
            s.push(footer_pipe());
            s.extend(footer_cmd("↑↓", "History"));
            s.push(footer_pipe());
            s.extend(footer_cmd("Ctrl+C", "Quit"));
        }
        AppMode::Running => {
            s.extend(footer_cmd("Esc", "Cancel"));
            s.push(footer_pipe());
            s.extend(footer_cmd("q", "Quit"));
            s.push(footer_pipe());
            s.extend(footer_cmd("?", "Help"));
        }
        AppMode::Selecting(state) => {
            let n = state.selected.len();
            s.extend(footer_cmd("Space", "Toggle"));
            s.push(footer_pipe());
            s.extend(footer_cmd("a", "All"));
            s.push(footer_pipe());
            s.extend(footer_cmd("n", "None"));
            s.push(footer_pipe());
            s.extend(footer_cmd("c", "Copy"));
            s.push(footer_pipe());
            if state.refresh_in_flight {
                let spinner = SPINNER_FRAMES[app.tick as usize % SPINNER_FRAMES.len()];
                s.push(Span::styled(
                    format!("{spinner} Loading..."),
                    Style::default().fg(THEME.info),
                ));
            } else {
                s.extend(footer_cmd("r", "More"));
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
        AppMode::Done { note_ids, .. } => {
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

    let input_area = v_chunks[1];
    let inner_width = input_area.width.saturating_sub(2).max(1) as usize;
    let scroll = input.visual_scroll(inner_width);

    let mut block = Block::default()
        .borders(Borders::ALL)
        .title(" Enter term ")
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
    frame.render_widget(para, input_area);

    if show_cursor {
        frame.set_cursor_position((
            input_area.x + 1 + (input.visual_cursor().saturating_sub(scroll)) as u16,
            input_area.y + 1,
        ));
    }
}

fn draw_running(frame: &mut Frame, app: &App, area: Rect) {
    // Steps are in the sidebar; main area is just the log
    draw_log_panel(frame, app, area);
}

fn draw_log_panel(frame: &mut Frame, app: &App, area: Rect) {
    let visible_height = area.height.saturating_sub(2) as usize;
    let total_logs = app.logs.len();
    let scroll_pos = app.log_scroll as usize;
    // Show a window of logs ending at scroll_pos (inclusive)
    let end = (scroll_pos + 1).min(total_logs);
    let start = end.saturating_sub(visible_height);

    let log_text: Text = app.logs[start..end]
        .iter()
        .map(|l| Line::from(l.as_str()))
        .collect::<Vec<_>>()
        .into();

    let log_block = Block::default()
        .borders(Borders::ALL.difference(Borders::LEFT))
        .title(" Log ")
        .border_style(Style::default().fg(THEME.border));
    let log_para = Paragraph::new(log_text)
        .block(log_block)
        .wrap(Wrap { trim: false });
    frame.render_widget(log_para, area);
}

fn draw_selecting(frame: &mut Frame, state: &SelectionState, glyphs: &Glyphs, area: Rect) {
    // Split: card list on top, detail below
    let list_height = (state.cards.len() as u16 + 2).min(area.height / 2); // +2 for border
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(list_height), Constraint::Min(0)])
        .split(area);

    // Card list
    let list_items: Vec<ListItem> = state
        .cards
        .iter()
        .enumerate()
        .map(|(i, card)| {
            let checkbox = if card.is_duplicate {
                "  "
            } else if state.selected.contains(&i) {
                glyphs.checkbox_checked
            } else {
                glyphs.checkbox_unchecked
            };

            let label = card
                .anki_fields
                .values()
                .next()
                .map(|v| super::selector::strip_html_tags(v))
                .unwrap_or_default();
            let dup_note = if card.is_duplicate { " [dup]" } else { "" };
            let flag_note = if !card.flags.is_empty() {
                " [flagged]"
            } else {
                ""
            };

            let style = if i == state.cursor {
                Style::default()
                    .fg(THEME.highlight_fg)
                    .bg(THEME.highlight_bg)
                    .add_modifier(Modifier::BOLD)
            } else if card.is_duplicate {
                Style::default().fg(THEME.dimmed)
            } else if state.selected.contains(&i) {
                Style::default().fg(THEME.success)
            } else {
                Style::default()
            };

            // Keep checkbox un-bolded so Nerd Font glyphs render at correct size
            let checkbox_style = if i == state.cursor {
                Style::default()
                    .fg(THEME.highlight_fg)
                    .bg(THEME.highlight_bg)
            } else {
                style
            };
            let mut spans = vec![
                Span::styled(checkbox, checkbox_style),
                Span::styled(format!("{label}{dup_note}"), style),
            ];
            if !flag_note.is_empty() {
                spans.push(Span::styled(
                    flag_note,
                    Style::default()
                        .fg(THEME.warning)
                        .add_modifier(Modifier::DIM),
                ));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    let mut list_state = state.list_state;
    let title = format!(
        " Cards ({}/{} selected) ",
        state.selected.len(),
        state.cards.len()
    );
    let list = List::new(list_items).block(
        Block::default()
            .borders(Borders::BOTTOM)
            .title(title)
            .border_style(Style::default().fg(THEME.border)),
    );
    frame.render_stateful_widget(list, chunks[0], &mut list_state);

    // Detail pane for focused card
    if let Some(card) = state.cards.get(state.cursor) {
        let mut lines: Vec<Line> = Vec::new();

        if card.is_duplicate {
            lines.push(Line::from(Span::styled(
                "  ⚠ Already exists in Anki",
                Style::default().fg(THEME.warning),
            )));
            lines.push(Line::from(""));
        }

        if !card.flags.is_empty() {
            for flag in &card.flags {
                lines.push(Line::from(Span::styled(
                    format!("  ⚠ {flag}"),
                    Style::default()
                        .fg(THEME.warning)
                        .add_modifier(Modifier::DIM),
                )));
            }
            lines.push(Line::from(""));
        }

        for (name, value) in &card.raw_anki_fields {
            lines.push(Line::from(Span::styled(
                name.clone(),
                Style::default().fg(THEME.info).add_modifier(Modifier::BOLD),
            )));
            lines.extend(super::selector::markdown_to_lines(value, "  "));
            lines.push(Line::from(""));
        }

        let detail_para = Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: false })
            .scroll((state.detail_scroll, 0));
        frame.render_widget(detail_para, chunks[1]);
    }
}

fn draw_reviewing(frame: &mut Frame, state: &ReviewState, area: Rect) {
    if let Some(flagged) = state.current() {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(5)])
            .split(area);

        // Reason
        let reason_para = Paragraph::new(flagged.reason.as_str())
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Reason ")
                    .border_style(Style::default().fg(THEME.warning)),
            )
            .wrap(Wrap { trim: false });
        frame.render_widget(reason_para, chunks[0]);

        // Card detail
        let mut lines: Vec<Line> = Vec::new();
        for (name, value) in &flagged.card.raw_anki_fields {
            lines.push(Line::from(Span::styled(
                name.clone(),
                Style::default().fg(THEME.info).add_modifier(Modifier::BOLD),
            )));
            lines.extend(super::selector::markdown_to_lines(value, "  "));
            lines.push(Line::from(""));
        }

        let detail_para = Paragraph::new(Text::from(lines))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Card ")
                    .border_style(Style::default().fg(THEME.border)),
            )
            .wrap(Wrap { trim: false })
            .scroll((state.detail_scroll, 0));
        frame.render_widget(detail_para, chunks[1]);
    }
}

fn draw_done(frame: &mut Frame, app: &App, msg: &str, cards: &[ValidatedCard], area: Rect) {
    let mut summary_lines = vec![
        Line::from(Span::styled(
            "✓ Done",
            Style::default()
                .fg(THEME.success)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(msg),
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

fn draw_step_logs(frame: &mut Frame, app: &App, step_idx: usize, area: Rect) {
    let record = &app.steps[step_idx];
    let title = format!(" {} ", record.step.label());

    if record.logs.is_empty() {
        let lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                "  No log entries for this step.",
                Style::default().fg(THEME.dimmed),
            )),
        ];
        let para = Paragraph::new(Text::from(lines))
            .block(
                Block::default()
                    .borders(Borders::ALL.difference(Borders::LEFT))
                    .title(title)
                    .border_style(Style::default().fg(THEME.border)),
            )
            .wrap(Wrap { trim: false });
        frame.render_widget(para, area);
        return;
    }

    let log_text: Text = record
        .logs
        .iter()
        .map(|l| Line::from(l.as_str()))
        .collect::<Vec<_>>()
        .into();

    let log_block = Block::default()
        .borders(Borders::ALL.difference(Borders::LEFT))
        .title(title)
        .border_style(Style::default().fg(THEME.border));
    let log_para = Paragraph::new(log_text)
        .block(log_block)
        .wrap(Wrap { trim: false })
        .scroll((app.browse_scroll, 0));
    frame.render_widget(log_para, area);
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

// ---------------------------------------------------------------------------
// Prompt picker (shown before the main TUI when --prompt is omitted)
// ---------------------------------------------------------------------------

fn run_prompt_picker(
    mut terminal: DefaultTerminal,
    prompts: &[crate::workspace::discovery::PromptEntry],
    glyphs: &Glyphs,
) -> Option<PathBuf> {
    use crate::workspace::resolver::last_prompt;

    let mut cursor: usize = 0;
    let mut list_state = ListState::default();

    // Pre-select last-used prompt if it matches one of the entries
    if let Some(last) = last_prompt()
        && let Some(idx) = prompts.iter().position(|p| p.path == last)
    {
        cursor = idx;
    }
    list_state.select(Some(cursor));

    loop {
        terminal
            .draw(|f| draw_prompt_picker(f, prompts, cursor, &mut list_state, glyphs))
            .ok();

        if event::poll(Duration::from_millis(50)).unwrap_or(false)
            && let Ok(Event::Key(key)) = event::read()
        {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    if cursor > 0 {
                        cursor -= 1;
                        list_state.select(Some(cursor));
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if cursor + 1 < prompts.len() {
                        cursor += 1;
                        list_state.select(Some(cursor));
                    }
                }
                KeyCode::Enter => {
                    return Some(prompts[cursor].path.clone());
                }
                KeyCode::Esc | KeyCode::Char('q') => {
                    return None;
                }
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return None;
                }
                _ => {}
            }
        }
    }
}

fn draw_prompt_picker(
    frame: &mut Frame,
    prompts: &[crate::workspace::discovery::PromptEntry],
    cursor: usize,
    list_state: &mut ListState,
    _glyphs: &Glyphs,
) {
    let area = frame.area();

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(area);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[0]);

    // Left: prompt list
    let items: Vec<ListItem> = prompts
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let style = if i == cursor {
                Style::default().fg(THEME.info).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(THEME.text)
            };
            let deck_info = &p.deck;
            ListItem::new(Line::from(vec![
                Span::styled(&p.title, style),
                Span::styled(format!("  {deck_info}"), Style::default().fg(THEME.dimmed)),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(THEME.border))
                .title(Span::styled(
                    " Select Prompt ",
                    Style::default().fg(THEME.info).add_modifier(Modifier::BOLD),
                )),
        )
        .highlight_style(
            Style::default()
                .bg(THEME.highlight_bg)
                .fg(THEME.info)
                .add_modifier(Modifier::BOLD),
        );

    frame.render_stateful_widget(list, cols[0], list_state);

    // Right: detail panel for selected prompt
    let selected = &prompts[cursor];
    let mut detail_lines = vec![
        Line::from(vec![
            Span::styled("Title: ", Style::default().fg(THEME.dimmed)),
            Span::styled(&selected.title, Style::default().fg(THEME.text)),
        ]),
        Line::from(vec![
            Span::styled("Deck: ", Style::default().fg(THEME.dimmed)),
            Span::styled(selected.deck.as_str(), Style::default().fg(THEME.text)),
        ]),
        Line::from(vec![
            Span::styled("Note type: ", Style::default().fg(THEME.dimmed)),
            Span::styled(selected.note_type.as_str(), Style::default().fg(THEME.text)),
        ]),
    ];
    if let Some(ref desc) = selected.description {
        detail_lines.push(Line::from(""));
        detail_lines.push(Line::from(Span::styled(
            desc.as_str(),
            Style::default().fg(THEME.text),
        )));
    }
    detail_lines.push(Line::from(""));
    detail_lines.push(Line::from(Span::styled(
        selected.path.display().to_string(),
        Style::default().fg(THEME.dimmed),
    )));

    let detail = Paragraph::new(detail_lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(THEME.border))
                .title(Span::styled(" Details ", Style::default().fg(THEME.header))),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(detail, cols[1]);

    // Footer
    let footer = Line::from(vec![
        Span::styled("↑↓", Style::default().fg(THEME.dimmed)),
        Span::styled(
            " Navigate",
            Style::default().fg(THEME.text).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" │ ", Style::default().fg(THEME.border)),
        Span::styled("Enter", Style::default().fg(THEME.dimmed)),
        Span::styled(
            " Select",
            Style::default().fg(THEME.text).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" │ ", Style::default().fg(THEME.border)),
        Span::styled("Esc", Style::default().fg(THEME.dimmed)),
        Span::styled(
            " Quit",
            Style::default().fg(THEME.text).add_modifier(Modifier::BOLD),
        ),
    ]);
    frame.render_widget(Paragraph::new(footer), rows[1]);
}

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
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};

use crate::cli::GenerateArgs;
use crate::llm::pricing;

use super::cards::ValidatedCard;
use super::quality::FlaggedCard;

// ---------------------------------------------------------------------------
// Events / responses
// ---------------------------------------------------------------------------

pub struct SessionInfo {
    pub deck: String,
    pub note_type: String,
    pub model: String,
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
    RunDone(String),  // single run finished successfully
    RunError(String), // single run failed (can retry with new term)
    Fatal(String),    // session-level error (must exit)
}

pub enum WorkerCommand {
    Start(String), // term to generate cards for
    Refresh,       // generate more cards for the same term
    Selection(Vec<usize>),
    Review(Vec<bool>), // true = keep, false = discard
    Cancel,            // abandon current run, go back to input
    Quit,
}

// ---------------------------------------------------------------------------
// Pipeline steps
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PipelineStep {
    LoadPrompt,
    ValidateAnki,
    Generate,
    Validate,
    Select,
    QualityCheck,
    Finish,
}

impl PipelineStep {
    pub fn label(self) -> &'static str {
        match self {
            PipelineStep::LoadPrompt => "Load prompt",
            PipelineStep::ValidateAnki => "Validate Anki",
            PipelineStep::Generate => "Generate cards",
            PipelineStep::Validate => "Check duplicates",
            PipelineStep::Select => "Select cards",
            PipelineStep::QualityCheck => "Quality check",
            PipelineStep::Finish => "Import / export",
        }
    }
}

const ALL_STEPS: &[PipelineStep] = &[
    PipelineStep::LoadPrompt,
    PipelineStep::ValidateAnki,
    PipelineStep::Generate,
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
    #[allow(dead_code)]
    Error(String),
}

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

enum AppMode {
    Input(String), // term text being typed
    Running,
    Selecting(SelectionState),
    Reviewing(ReviewState),
    Done(String),
    Error(String),
}

struct SelectionState {
    cards: Vec<ValidatedCard>,
    cursor: usize,
    selected: BTreeSet<usize>,
    list_state: ListState,
    detail_scroll: u16,
    /// Tick when card was last copied to clipboard (for flash feedback).
    copied_at: Option<u64>,
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
            copied_at: None,
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

    fn is_done(&self) -> bool {
        self.cursor >= self.flagged.len()
    }
}

const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

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
}

struct App {
    mode: AppMode,
    session_info: Option<SessionInfo>,
    logs: Vec<String>,
    steps: Vec<(PipelineStep, StepStatus)>,
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
    /// Last term submitted, for retry.
    last_term: Option<String>,
    /// True after a Fatal error — worker is dead, no new runs possible.
    is_fatal: bool,
    history: InputHistory,
    backend_rx: mpsc::Receiver<BackendEvent>,
    worker_tx: mpsc::SyncSender<WorkerCommand>,
}

impl App {
    fn new(
        initial_term: Option<String>,
        backend_rx: mpsc::Receiver<BackendEvent>,
        worker_tx: mpsc::SyncSender<WorkerCommand>,
    ) -> Self {
        let steps = ALL_STEPS
            .iter()
            .map(|&s| (s, StepStatus::Pending))
            .collect();
        let last_term = initial_term.clone();
        let mode = if let Some(term) = initial_term {
            worker_tx.send(WorkerCommand::Start(term)).ok();
            AppMode::Running
        } else {
            AppMode::Input(String::new())
        };
        App {
            mode,
            session_info: None,
            logs: Vec::new(),
            steps,
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
            last_term,
            is_fatal: false,
            history: InputHistory::load(),
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
        for (_, status) in &mut self.steps {
            *status = StepStatus::Pending;
        }
    }

    fn step_status_mut(&mut self, step: PipelineStep) -> Option<&mut StepStatus> {
        self.steps
            .iter_mut()
            .find(|(s, _)| *s == step)
            .map(|(_, st)| st)
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
            if matches!(event, BackendEvent::RunDone(_) | BackendEvent::RunError(_)) {
                self.pending_cancels -= 1;
            }
            return;
        }

        match event {
            BackendEvent::SessionReady(_) => unreachable!(),
            BackendEvent::Log(msg) => {
                self.logs.push(msg);
                if self.log_auto_scroll {
                    self.log_scroll = self.logs.len().saturating_sub(1) as u16;
                }
            }
            BackendEvent::StepUpdate { step, status } => {
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
            BackendEvent::RunDone(msg) => {
                self.mode = AppMode::Done(msg);
            }
            BackendEvent::RunError(msg) => {
                self.mode = AppMode::Error(msg);
            }
            BackendEvent::Fatal(msg) => {
                self.mode = AppMode::Error(msg);
                self.is_fatal = true;
            }
        }
    }

    fn handle_key(&mut self, key: crossterm::event::KeyEvent) {
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
                    self.mode = AppMode::Input(String::new());
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
            AppMode::Done(_) | AppMode::Error(_) => match key.code {
                KeyCode::Char('n') if !self.is_fatal => {
                    self.reset_for_new_run();
                    self.mode = AppMode::Input(String::new());
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
                _ => {}
            },
        }
    }

    fn handle_key_input(&mut self, key: crossterm::event::KeyEvent) {
        let AppMode::Input(ref mut text) = self.mode else {
            return;
        };

        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.worker_tx.send(WorkerCommand::Quit).ok();
                self.should_quit = true;
                self.user_quit = true;
            }
            KeyCode::Esc => {
                if !text.is_empty() {
                    text.clear();
                }
            }
            KeyCode::Char(c) => {
                self.history.reset_browse();
                text.push(c);
            }
            KeyCode::Backspace => {
                self.history.reset_browse();
                text.pop();
            }
            KeyCode::Up => {
                if let Some(entry) = self.history.up(text) {
                    *text = entry.to_string();
                }
            }
            KeyCode::Down => {
                if let Some(entry) = self.history.down() {
                    *text = entry.to_string();
                }
            }
            KeyCode::Enter => {
                let term = text.trim().to_string();
                if !term.is_empty() {
                    self.history.push(&term);
                    self.history.reset_browse();
                    self.last_term = Some(term.clone());
                    self.mode = AppMode::Running;
                    self.worker_tx.send(WorkerCommand::Start(term)).ok();
                }
            }
            _ => {}
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
            KeyCode::Esc => {
                let was_refreshing = state.refresh_in_flight;
                self.worker_tx.send(WorkerCommand::Cancel).ok();
                if was_refreshing {
                    self.pending_cancels += 1;
                }
                self.reset_for_new_run();
                self.mode = AppMode::Input(String::new());
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
                let card = &state.cards[state.cursor];
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
                state.copied_at = Some(self.tick);
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

        match key.code {
            KeyCode::Char('k') | KeyCode::Char('y') | KeyCode::Enter => {
                state.keep_current();
            }
            KeyCode::Char('d') | KeyCode::Char('n') => {
                state.discard_current();
            }
            KeyCode::Char('a') => {
                state.keep_all();
            }
            KeyCode::Char('x') => {
                state.discard_all();
            }
            KeyCode::Char('q') => {
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
        AppMode::Input(text) => draw_input(frame, text, main_area),
        AppMode::Running => draw_running(frame, app, main_area),
        AppMode::Selecting(state) => draw_selecting(frame, state, main_area),
        AppMode::Reviewing(state) => draw_reviewing(frame, state, main_area),
        AppMode::Done(msg) => draw_done(frame, app, msg, main_area),
        AppMode::Error(msg) => draw_error(frame, msg, main_area),
    }

    draw_sidebar(frame, app, cols[0]);
    draw_footer(frame, app, rows[1]);
}

fn draw_sidebar(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(THEME.border));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let info_height: u16 = if app.session_info.is_some() { 4 } else { 0 };

    // Build step lines (detail on second indented line)
    let spinner_frame = format!(
        "{} ",
        SPINNER_FRAMES[app.tick as usize % SPINNER_FRAMES.len()]
    );

    let mut step_lines: Vec<Line> = Vec::new();
    for (step, status) in &app.steps {
        let is_interactive = matches!(step, PipelineStep::Select | PipelineStep::QualityCheck);
        let (icon, style): (&str, Style) = match status {
            StepStatus::Pending => ("  ", Style::default().fg(THEME.dimmed)),
            StepStatus::Running(_) if is_interactive => ("▸ ", Style::default().fg(THEME.info)),
            StepStatus::Running(_) => (&spinner_frame, Style::default().fg(THEME.info)),
            StepStatus::Done(_) => ("✓ ", Style::default().fg(THEME.success)),
            StepStatus::Skipped => ("- ", Style::default().fg(THEME.dimmed)),
            StepStatus::Error(_) => ("✗ ", Style::default().fg(THEME.danger)),
        };

        let detail = match status {
            StepStatus::Running(Some(d)) | StepStatus::Done(Some(d)) => Some(d.as_str()),
            StepStatus::Error(e) => Some(e.as_str()),
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
                    Span::styled(format!("  {d}"), Style::default().fg(THEME.dimmed)),
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
        let lines = vec![
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
            s.extend(footer_cmd("↑↓", "History"));
            s.push(footer_pipe());
            s.extend(footer_cmd("q", "Quit"));
        }
        AppMode::Running => {
            s.extend(footer_cmd("Esc", "Cancel"));
            s.push(footer_pipe());
            s.extend(footer_cmd("q", "Quit"));
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
            s.extend(footer_cmd("Enter", "Confirm"));
            s.push(footer_pipe());
            s.extend(footer_cmd("Esc", "Back"));
            s.push(footer_pipe());
            s.extend(footer_cmd("q", "Quit"));
            s.push(Span::styled(
                format!("  ({n} selected)"),
                Style::default().fg(THEME.dimmed),
            ));
            let copied = state
                .copied_at
                .is_some_and(|t| app.tick.wrapping_sub(t) < 20);
            if copied {
                s.push(Span::styled(
                    "  Copied!",
                    Style::default().fg(THEME.success),
                ));
            }
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
            s.extend(footer_cmd("a", "Keep all"));
            s.push(footer_pipe());
            s.extend(footer_cmd("x", "Discard all"));
            s.push(footer_pipe());
            s.extend(footer_cmd("q", "Quit"));
        }
        AppMode::Done(_) | AppMode::Error(_) => {
            if !app.is_fatal {
                s.extend(footer_cmd("n", "New term"));
                if app.last_term.is_some() {
                    s.push(footer_pipe());
                    s.extend(footer_cmd("r", "Retry"));
                }
                s.push(footer_pipe());
            }
            s.extend(footer_cmd("q", "Quit"));
        }
    }

    frame.render_widget(Paragraph::new(Line::from(s)), area);
}

fn draw_input(frame: &mut Frame, text: &str, area: Rect) {
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
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Enter term ")
        .border_style(Style::default().fg(THEME.info));

    let para = Paragraph::new(text).block(block);
    frame.render_widget(para, input_area);

    frame.set_cursor_position((input_area.x + 1 + text.len() as u16, input_area.y + 1));
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

fn draw_selecting(frame: &mut Frame, state: &SelectionState, area: Rect) {
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
                "\u{f4a7} "
            } else {
                "\u{f0131} "
            };

            let label = card
                .anki_fields
                .values()
                .next()
                .map(|v| super::selector::strip_html_tags(v))
                .unwrap_or_default();
            let dup_note = if card.is_duplicate { " [dup]" } else { "" };

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
            ListItem::new(Line::from(vec![
                Span::styled(checkbox, checkbox_style),
                Span::styled(format!("{label}{dup_note}"), style),
            ]))
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

fn draw_done(frame: &mut Frame, app: &App, msg: &str, area: Rect) {
    let mut lines = vec![
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
        lines.push(Line::from(""));
        lines.push(Line::from(format!(
            "Tokens: {} in / {} out  |  Cost: {}",
            app.run_input_tokens,
            app.run_output_tokens,
            pricing::format_cost(app.run_cost)
        )));
        if app.session_cost > 0.0 {
            lines.push(Line::from(format!(
                "Session total: {}",
                pricing::format_cost(app.session_cost + app.run_cost)
            )));
        }
    }

    let para = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
    frame.render_widget(para, area);
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

/// Returns `true` if the user explicitly quit (q / Ctrl-C), `false` for natural Done/Error exit.
fn run_app(
    mut terminal: DefaultTerminal,
    initial_term: Option<String>,
    backend_rx: mpsc::Receiver<BackendEvent>,
    worker_tx: mpsc::SyncSender<WorkerCommand>,
) -> anyhow::Result<bool> {
    let mut app = App::new(initial_term, backend_rx, worker_tx);

    loop {
        app.tick = app.tick.wrapping_add(1);
        terminal.draw(|f| draw(f, &app))?;

        // Drain all pending backend events
        loop {
            match app.backend_rx.try_recv() {
                Ok(ev) => app.handle_backend_event(ev),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    if !matches!(app.mode, AppMode::Done(_) | AppMode::Error(_)) {
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
        if event::poll(Duration::from_millis(50))?
            && let Event::Key(key) = event::read()?
        {
            app.handle_key(key);
        }

        if app.should_quit {
            break;
        }
    }

    let user_quit = app.user_quit;
    Ok(user_quit)
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

pub fn run_tui(args: GenerateArgs) -> anyhow::Result<()> {
    let initial_term = args.term.clone();

    let (tx_events, rx_events) = mpsc::channel::<BackendEvent>();
    // Capacity 10 so the TUI can send Start/Quit without blocking even when
    // the worker isn't currently waiting on the command channel.
    let (tx_cmd, rx_cmd) = mpsc::sync_channel::<WorkerCommand>(10);

    // Spawn worker thread
    let worker_handle =
        std::thread::spawn(move || super::command_generate::run_pipeline(args, tx_events, rx_cmd));

    // Run TUI on main thread
    let terminal = ratatui::init();
    let user_quit = run_app(terminal, initial_term, rx_events, tx_cmd).unwrap_or(true);
    ratatui::restore();

    if user_quit {
        // Worker may be blocked on an LLM call — don't hang waiting for it.
        std::process::exit(0);
    }

    // Natural finish: propagate any worker error to the process exit code.
    worker_handle
        .join()
        .unwrap_or_else(|_| Err(anyhow::anyhow!("Worker thread panicked")))
}

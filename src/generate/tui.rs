use std::collections::BTreeSet;
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
    tick: u64,
    /// Counts how many runs have been cancelled. While > 0, backend events are
    /// discarded. Decremented when RunDone/RunError arrives from a cancelled run.
    pending_cancels: u32,
    should_quit: bool,
    /// True when the user explicitly pressed q/Ctrl-C (as opposed to natural Done/Error exit).
    user_quit: bool,
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
            tick: 0,
            pending_cancels: 0,
            should_quit: false,
            user_quit: false,
            backend_rx,
            worker_tx,
        }
    }

    fn reset_for_new_run(&mut self) {
        self.logs.clear();
        self.log_scroll = 0;
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
                // Auto-scroll to bottom
                self.log_scroll = self.logs.len().saturating_sub(1) as u16;
            }
            BackendEvent::StepUpdate { step, status } => {
                if let Some(st) = self.step_status_mut(step) {
                    *st = status;
                }
            }
            BackendEvent::RequestSelection(cards) => {
                self.mode = AppMode::Selecting(SelectionState::new(cards));
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
                // Fatal means the worker is dead, no point continuing
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
                _ => {}
            },
            AppMode::Selecting(_) => self.handle_key_selection(key),
            AppMode::Reviewing(_) => self.handle_key_review(key),
            AppMode::Done(_) | AppMode::Error(_) => match key.code {
                KeyCode::Char('n') => {
                    self.reset_for_new_run();
                    self.mode = AppMode::Input(String::new());
                }
                _ => {
                    self.worker_tx.send(WorkerCommand::Quit).ok();
                    self.should_quit = true;
                }
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
                self.worker_tx.send(WorkerCommand::Quit).ok();
                self.should_quit = true;
                self.user_quit = true;
            }
            KeyCode::Char(c) => text.push(c),
            KeyCode::Backspace => {
                text.pop();
            }
            KeyCode::Enter => {
                let term = text.trim().to_string();
                if !term.is_empty() {
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
            KeyCode::Esc => {
                self.worker_tx.send(WorkerCommand::Cancel).ok();
                self.reset_for_new_run();
                self.mode = AppMode::Input(String::new());
            }
            KeyCode::Char('q') => {
                self.worker_tx.send(WorkerCommand::Quit).ok();
                self.should_quit = true;
                self.user_quit = true;
            }
            KeyCode::Enter => {
                let AppMode::Selecting(state) = std::mem::replace(&mut self.mode, AppMode::Running)
                else {
                    return;
                };
                let indices: Vec<usize> = state.selected.into_iter().collect();
                self.worker_tx.send(WorkerCommand::Selection(indices)).ok();
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
            KeyCode::Char('q') | KeyCode::Esc => {
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
    let block = Block::default().borders(Borders::RIGHT);
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
            StepStatus::Pending => ("  ", Style::default().fg(Color::DarkGray)),
            StepStatus::Running(_) if is_interactive => ("▸ ", Style::default().fg(Color::Cyan)),
            StepStatus::Running(_) => (&spinner_frame, Style::default().fg(Color::Cyan)),
            StepStatus::Done(_) => ("✓ ", Style::default().fg(Color::Green)),
            StepStatus::Skipped => ("- ", Style::default().fg(Color::DarkGray)),
            StepStatus::Error(_) => ("✗ ", Style::default().fg(Color::Red)),
        };

        step_lines.push(Line::from(vec![
            Span::styled(icon, style),
            Span::styled(step.label(), style),
        ]));

        let detail = match status {
            StepStatus::Running(Some(d)) | StepStatus::Done(Some(d)) => Some(d.as_str()),
            StepStatus::Error(e) => Some(e.as_str()),
            _ => None,
        };
        if let Some(d) = detail {
            step_lines.push(Line::from(Span::styled(
                format!("    {d}"),
                Style::default().fg(Color::DarkGray),
            )));
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
                Span::styled("Deck  ", Style::default().fg(Color::DarkGray)),
                Span::raw(&info.deck),
            ]),
            Line::from(vec![
                Span::styled("Note  ", Style::default().fg(Color::DarkGray)),
                Span::raw(&info.note_type),
            ]),
            Line::from(vec![
                Span::styled("Model ", Style::default().fg(Color::DarkGray)),
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
                Style::default().fg(Color::DarkGray),
            ))),
            chunks[3],
        );
    }
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let hints = match &app.mode {
        AppMode::Input(_) => " [Enter] generate  [q] quit".to_string(),
        AppMode::Running => " [Esc] cancel  [q] quit".to_string(),
        AppMode::Selecting(state) => {
            let n = state.selected.len();
            format!(
                " [Space] toggle  [a] all  [n] none  [Enter] confirm  [Esc] back  [q] quit  ({n} selected)"
            )
        }
        AppMode::Reviewing(state) => {
            let cur = (state.cursor + 1).min(state.flagged.len());
            let total = state.flagged.len();
            format!(
                " Flagged {cur}/{total}  [k] keep  [d] discard  [a] keep all  [x] discard all  [q] quit"
            )
        }
        AppMode::Done(_) | AppMode::Error(_) => " [n] new term  [any key] quit".to_string(),
    };

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            hints,
            Style::default().fg(Color::DarkGray),
        ))),
        area,
    );
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
        .border_style(Style::default().fg(Color::Cyan));

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
    let start = total_logs.saturating_sub(visible_height);

    let log_text: Text = app.logs[start..]
        .iter()
        .map(|l| Line::from(l.as_str()))
        .collect::<Vec<_>>()
        .into();

    let log_block = Block::default().borders(Borders::ALL).title(" Log ");
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
                "☑ "
            } else {
                "☐ "
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
                    .fg(Color::Black)
                    .bg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else if card.is_duplicate {
                Style::default().fg(Color::DarkGray)
            } else if state.selected.contains(&i) {
                Style::default().fg(Color::Green)
            } else {
                Style::default()
            };

            ListItem::new(Line::from(vec![
                Span::styled(checkbox, style),
                Span::styled(format!("{label}{dup_note}"), style),
            ]))
        })
        .collect();

    let mut list_state = state.list_state;
    let list = List::new(list_items).block(Block::default().borders(Borders::BOTTOM));
    frame.render_stateful_widget(list, chunks[0], &mut list_state);

    // Detail pane for focused card
    if let Some(card) = state.cards.get(state.cursor) {
        let mut lines: Vec<Line> = Vec::new();

        if card.is_duplicate {
            lines.push(Line::from(Span::styled(
                "  ⚠ Already exists in Anki",
                Style::default().fg(Color::Yellow),
            )));
            lines.push(Line::from(""));
        }

        for (name, value) in &card.raw_anki_fields {
            lines.push(Line::from(Span::styled(
                name.clone(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
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
                    .border_style(Style::default().fg(Color::Yellow)),
            )
            .wrap(Wrap { trim: false });
        frame.render_widget(reason_para, chunks[0]);

        // Card detail
        let mut lines: Vec<Line> = Vec::new();
        for (name, value) in &flagged.card.raw_anki_fields {
            lines.push(Line::from(Span::styled(
                name.clone(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )));
            lines.extend(super::selector::markdown_to_lines(value, "  "));
            lines.push(Line::from(""));
        }

        let detail_para = Paragraph::new(Text::from(lines))
            .block(Block::default().borders(Borders::ALL).title(" Card "))
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
                .fg(Color::Green)
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
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(msg, Style::default().fg(Color::Red))),
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

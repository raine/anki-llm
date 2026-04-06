use std::collections::BTreeSet;
use std::sync::mpsc;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout};
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

pub enum BackendEvent {
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
    Done(String),  // success message
    Error(String), // fatal error
}

pub enum WorkerResponse {
    Selection(Vec<usize>),
    Review(Vec<bool>), // true = keep, false = discard
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
        // Pre-select all non-duplicate cards
        let selected = cards
            .iter()
            .enumerate()
            .filter(|(_, c)| !c.is_duplicate)
            .map(|(i, _)| i)
            .collect();
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

struct App {
    mode: AppMode,
    logs: Vec<String>,
    steps: Vec<(PipelineStep, StepStatus)>,
    total_cost: f64,
    input_tokens: u64,
    output_tokens: u64,
    log_scroll: u16,
    should_quit: bool,
    backend_rx: mpsc::Receiver<BackendEvent>,
    worker_tx: mpsc::SyncSender<WorkerResponse>,
}

impl App {
    fn new(
        backend_rx: mpsc::Receiver<BackendEvent>,
        worker_tx: mpsc::SyncSender<WorkerResponse>,
    ) -> Self {
        let steps = ALL_STEPS
            .iter()
            .map(|&s| (s, StepStatus::Pending))
            .collect();
        App {
            mode: AppMode::Running,
            logs: Vec::new(),
            steps,
            total_cost: 0.0,
            input_tokens: 0,
            output_tokens: 0,
            log_scroll: 0,
            should_quit: false,
            backend_rx,
            worker_tx,
        }
    }

    fn step_status_mut(&mut self, step: PipelineStep) -> Option<&mut StepStatus> {
        self.steps
            .iter_mut()
            .find(|(s, _)| *s == step)
            .map(|(_, st)| st)
    }

    fn handle_backend_event(&mut self, event: BackendEvent) {
        match event {
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
                self.input_tokens += input_tokens;
                self.output_tokens += output_tokens;
                self.total_cost += cost;
            }
            BackendEvent::Done(msg) => {
                self.mode = AppMode::Done(msg);
            }
            BackendEvent::Error(msg) => {
                self.mode = AppMode::Error(msg);
            }
        }
    }

    fn handle_key(&mut self, key: crossterm::event::KeyEvent) {
        match &mut self.mode {
            AppMode::Running => {
                if key.code == KeyCode::Char('q')
                    || (key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL))
                {
                    // Signal worker to stop
                    self.worker_tx.send(WorkerResponse::Quit).ok();
                    self.should_quit = true;
                }
            }
            AppMode::Selecting(_) => self.handle_key_selection(key),
            AppMode::Reviewing(_) => self.handle_key_review(key),
            AppMode::Done(_) | AppMode::Error(_) => {
                // Any key exits
                self.should_quit = true;
            }
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
            KeyCode::Char('q') | KeyCode::Esc => {
                self.worker_tx.send(WorkerResponse::Quit).ok();
                self.should_quit = true;
            }
            KeyCode::Enter => {
                let AppMode::Selecting(state) = std::mem::replace(&mut self.mode, AppMode::Running)
                else {
                    return;
                };
                let indices: Vec<usize> = state.selected.into_iter().collect();
                self.worker_tx.send(WorkerResponse::Selection(indices)).ok();
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
            KeyCode::Char('k') | KeyCode::Char('y') => {
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
                self.worker_tx.send(WorkerResponse::Quit).ok();
                self.should_quit = true;
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
            self.worker_tx.send(WorkerResponse::Review(decisions)).ok();
        }
    }
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

fn draw(frame: &mut Frame, app: &App) {
    match &app.mode {
        AppMode::Running => draw_running(frame, app),
        AppMode::Selecting(state) => draw_selecting(frame, app, state),
        AppMode::Reviewing(state) => draw_reviewing(frame, state),
        AppMode::Done(msg) => draw_done(frame, app, msg),
        AppMode::Error(msg) => draw_error(frame, msg),
    }
}

fn draw_running(frame: &mut Frame, app: &App) {
    let area = frame.area();

    // Split: top for steps, bottom for logs
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(10), Constraint::Percentage(55)])
        .split(area);

    // Steps panel
    let step_items: Vec<ListItem> = app
        .steps
        .iter()
        .map(|(step, status)| {
            let (icon, style) = match status {
                StepStatus::Pending => ("  ", Style::default().fg(Color::DarkGray)),
                StepStatus::Running(_) => ("⟳ ", Style::default().fg(Color::Yellow)),
                StepStatus::Done(_) => ("✓ ", Style::default().fg(Color::Green)),
                StepStatus::Skipped => ("- ", Style::default().fg(Color::DarkGray)),
                StepStatus::Error(_) => ("✗ ", Style::default().fg(Color::Red)),
            };

            let detail_text = match status {
                StepStatus::Running(Some(d)) | StepStatus::Done(Some(d)) => {
                    format!("  {}", d)
                }
                StepStatus::Error(e) => format!("  {}", e),
                _ => String::new(),
            };

            let label = step.label();
            let line = if detail_text.is_empty() {
                Line::from(vec![Span::styled(icon, style), Span::styled(label, style)])
            } else {
                Line::from(vec![
                    Span::styled(icon, style),
                    Span::styled(label, style),
                    Span::styled(detail_text, Style::default().fg(Color::DarkGray)),
                ])
            };
            ListItem::new(line)
        })
        .collect();

    let cost_title = if app.total_cost > 0.0 {
        format!(" Steps — {} ", pricing::format_cost(app.total_cost))
    } else {
        " Steps ".to_string()
    };

    let steps_block = Block::default().borders(Borders::ALL).title(cost_title);
    let steps_list = List::new(step_items).block(steps_block);
    frame.render_widget(steps_list, chunks[0]);

    // Log panel
    draw_log_panel(frame, app, chunks[1]);
}

fn draw_log_panel(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
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

fn draw_selecting(frame: &mut Frame, _app: &App, state: &SelectionState) {
    let area = frame.area();

    let selected_count = state.selected.len();
    let title = format!(
        " Select cards  [Space] toggle  [a] all  [n] none  [Enter] confirm  [q] quit  ({selected_count} selected) "
    );

    let main_block = Block::default().borders(Borders::ALL).title(title);
    let inner = main_block.inner(area);
    frame.render_widget(main_block, area);

    // Split: left list, right detail
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(38), Constraint::Percentage(62)])
        .split(inner);

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

            let first_field = card.anki_fields.values().next().map(|v| {
                let plain = super::selector::strip_html_tags(v);
                if plain.len() > 35 {
                    let boundary = plain
                        .char_indices()
                        .map(|(idx, _)| idx)
                        .take_while(|&idx| idx <= 32)
                        .last()
                        .unwrap_or(0);
                    format!("{}...", &plain[..boundary])
                } else {
                    plain
                }
            });

            let label = first_field.unwrap_or_default();
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
    let list = List::new(list_items).block(Block::default().borders(Borders::RIGHT));
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
            // Render markdown as plain text with basic formatting
            let plain = super::selector::strip_html_tags(value);
            for l in plain.lines() {
                lines.push(Line::from(format!("  {l}")));
            }
            lines.push(Line::from(""));
        }

        let detail_para = Paragraph::new(Text::from(lines))
            .block(Block::default().borders(Borders::NONE))
            .wrap(Wrap { trim: false })
            .scroll((state.detail_scroll, 0));
        frame.render_widget(detail_para, chunks[1]);
    }
}

fn draw_reviewing(frame: &mut Frame, state: &ReviewState) {
    let area = frame.area();

    let total = state.flagged.len();
    let current_num = (state.cursor + 1).min(total);

    let title = format!(
        " Quality check — Flagged {current_num}/{total}  [k] keep  [d] discard  [a] keep all  [x] discard all  [q] quit "
    );

    let outer_block = Block::default().borders(Borders::ALL).title(title);
    let inner = outer_block.inner(area);
    frame.render_widget(outer_block, area);

    if let Some(flagged) = state.current() {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(5)])
            .split(inner);

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
            let plain = super::selector::strip_html_tags(value);
            for l in plain.lines() {
                lines.push(Line::from(format!("  {l}")));
            }
            lines.push(Line::from(""));
        }

        let detail_para = Paragraph::new(Text::from(lines))
            .block(Block::default().borders(Borders::ALL).title(" Card "))
            .wrap(Wrap { trim: false })
            .scroll((state.detail_scroll, 0));
        frame.render_widget(detail_para, chunks[1]);
    }
}

fn draw_done(frame: &mut Frame, app: &App, msg: &str) {
    let area = frame.area();
    let mut lines = vec![
        Line::from(Span::styled(
            "✓ Done",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(msg),
        Line::from(""),
    ];

    if app.total_cost > 0.0 {
        lines.push(Line::from(format!(
            "Tokens: {} in / {} out  |  Cost: {}",
            app.input_tokens,
            app.output_tokens,
            pricing::format_cost(app.total_cost)
        )));
        lines.push(Line::from(""));
    }

    lines.push(Line::from(Span::styled(
        "Press any key to exit",
        Style::default().fg(Color::DarkGray),
    )));

    let para = Paragraph::new(Text::from(lines))
        .block(Block::default().borders(Borders::ALL).title(" anki-llm "))
        .wrap(Wrap { trim: false });
    frame.render_widget(para, area);
}

fn draw_error(frame: &mut Frame, msg: &str) {
    let area = frame.area();
    let lines = vec![
        Line::from(Span::styled(
            "✗ Error",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(msg, Style::default().fg(Color::Red))),
        Line::from(""),
        Line::from(Span::styled(
            "Press any key to exit",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let para = Paragraph::new(Text::from(lines))
        .block(Block::default().borders(Borders::ALL).title(" anki-llm "))
        .wrap(Wrap { trim: false });
    frame.render_widget(para, area);
}

// ---------------------------------------------------------------------------
// Main event loop
// ---------------------------------------------------------------------------

fn run_app(
    mut terminal: DefaultTerminal,
    backend_rx: mpsc::Receiver<BackendEvent>,
    worker_tx: mpsc::SyncSender<WorkerResponse>,
) -> anyhow::Result<()> {
    let mut app = App::new(backend_rx, worker_tx);

    loop {
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

    Ok(())
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

pub fn run_tui(args: GenerateArgs) -> anyhow::Result<()> {
    let (tx_events, rx_events) = mpsc::channel::<BackendEvent>();
    // SyncSender with capacity 0 so the worker blocks until the TUI reads
    let (tx_response, rx_response) = mpsc::sync_channel::<WorkerResponse>(0);

    // Spawn worker thread
    let worker_handle = std::thread::spawn(move || {
        super::command_generate::run_pipeline(args, tx_events, rx_response)
    });

    // Run TUI on main thread
    let terminal = ratatui::init();
    let result = run_app(terminal, rx_events, tx_response);
    ratatui::restore();

    // Wait for worker; surface any worker error if TUI loop was clean
    let worker_result = worker_handle
        .join()
        .unwrap_or_else(|_| Err(anyhow::anyhow!("Worker thread panicked")));

    result.and(worker_result)
}

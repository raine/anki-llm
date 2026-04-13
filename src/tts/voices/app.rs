//! Ratatui event loop for the voice browser.
//!
//! Layout: left pane is a filtered voice list with a search input at
//! the top; right pane shows the highlighted voice's details and the
//! YAML scaffold that will be emitted on Enter.

use std::process::Child;
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};

use crate::tts::cache::TtsCache;
use crate::tui::line_input::LineInput;
use crate::tui::theme::{THEME, footer_cmd, footer_pipe};

use super::catalog::{ProviderId, VoiceEntry, filter, load_snapshot};
use super::credentials::{ProviderPreviewState, probe_all};
use super::player;
use super::preview::{PreviewHandle, PreviewRequest, PreviewResult, RequestId, spawn_worker};
use super::yaml::emit_scaffold;

pub struct InitialFilters {
    pub lang: Option<String>,
    pub provider: Option<ProviderId>,
    pub query: Option<String>,
}

pub struct AppOutcome {
    pub yaml: String,
    pub voice_id: String,
}

pub struct App {
    entries: Vec<VoiceEntry>,
    filtered: Vec<usize>,
    search: LineInput,
    list_state: ListState,
    lang_filter: Option<String>,
    provider_filter: Option<ProviderId>,
    provider_states: std::collections::HashMap<ProviderId, ProviderPreviewState>,
    cache: Arc<TtsCache>,
    worker: PreviewHandle,
    next_id: RequestId,
    current_id: RequestId,
    preview_busy: bool,
    queued: Option<usize>,
    active_player: Option<Child>,
    status_line: String,
    outcome: Option<AppOutcome>,
    should_quit: bool,
}

impl App {
    pub fn new(filters: InitialFilters, cache: Arc<TtsCache>) -> Self {
        let entries = load_snapshot();
        let provider_states = probe_all();
        let worker = spawn_worker();
        let mut app = Self {
            entries,
            filtered: Vec::new(),
            search: LineInput::new(filters.query.unwrap_or_default()),
            list_state: ListState::default(),
            lang_filter: filters.lang,
            provider_filter: filters.provider,
            provider_states,
            cache,
            worker,
            next_id: 0,
            current_id: 0,
            preview_busy: false,
            queued: None,
            active_player: None,
            status_line: "Type to search · Space=preview · Enter=copy+emit · q=cancel".into(),
            outcome: None,
            should_quit: false,
        };
        app.refilter();
        app
    }

    fn refilter(&mut self) {
        self.filtered = filter(
            &self.entries,
            self.search.value(),
            self.lang_filter.as_deref(),
            self.provider_filter,
        );
        let sel = if self.filtered.is_empty() {
            None
        } else {
            Some(0)
        };
        self.list_state.select(sel);
    }

    fn selected_index(&self) -> Option<usize> {
        self.list_state
            .selected()
            .and_then(|i| self.filtered.get(i).copied())
    }

    fn selected_entry(&self) -> Option<&VoiceEntry> {
        self.selected_index().map(|i| &self.entries[i])
    }

    fn move_up(&mut self) {
        if self.filtered.is_empty() {
            return;
        }
        let cur = self.list_state.selected().unwrap_or(0);
        let next = cur.saturating_sub(1);
        self.list_state.select(Some(next));
    }

    fn move_down(&mut self) {
        if self.filtered.is_empty() {
            return;
        }
        let cur = self.list_state.selected().unwrap_or(0);
        let next = (cur + 1).min(self.filtered.len().saturating_sub(1));
        self.list_state.select(Some(next));
    }

    fn page_up(&mut self, rows: usize) {
        if self.filtered.is_empty() {
            return;
        }
        let cur = self.list_state.selected().unwrap_or(0);
        self.list_state.select(Some(cur.saturating_sub(rows)));
    }

    fn page_down(&mut self, rows: usize) {
        if self.filtered.is_empty() {
            return;
        }
        let cur = self.list_state.selected().unwrap_or(0);
        let max = self.filtered.len().saturating_sub(1);
        self.list_state.select(Some((cur + rows).min(max)));
    }

    fn stop_player(&mut self) {
        if let Some(child) = self.active_player.take() {
            player::stop(child);
        }
    }

    fn reap_player(&mut self) {
        if let Some(child) = self.active_player.as_mut()
            && let Ok(Some(_)) = child.try_wait()
        {
            self.active_player = None;
        }
    }

    fn request_preview(&mut self) {
        self.stop_player();
        let Some(idx) = self.selected_index() else {
            return;
        };
        if self.preview_busy {
            self.queued = Some(idx);
            self.status_line = "Queued next preview…".into();
            return;
        }
        self.dispatch_preview(idx);
    }

    fn dispatch_preview(&mut self, idx: usize) {
        let entry = self.entries[idx].clone();
        let state = self
            .provider_states
            .get(&entry.provider)
            .cloned()
            .unwrap_or_else(|| ProviderPreviewState::Unavailable {
                reason: "unknown provider".into(),
            });
        self.next_id += 1;
        self.current_id = self.next_id;
        self.preview_busy = true;
        self.status_line = format!("Generating sample for {}…", entry.voice_id);
        self.worker.submit(PreviewRequest {
            id: self.current_id,
            entry,
            state,
            cache: Arc::clone(&self.cache),
        });
    }

    fn handle_preview_result(&mut self, result: PreviewResult) {
        let (id, outcome) = match result {
            PreviewResult::Ok { id, path } => (id, Ok(path)),
            PreviewResult::Err { id, message } => (id, Err(message)),
        };
        // If the user has moved on, drop this completion silently.
        if id != self.current_id {
            self.preview_busy = false;
            if let Some(queued) = self.queued.take() {
                self.dispatch_preview(queued);
            }
            return;
        }
        match outcome {
            Ok(path) => match player::spawn(&path) {
                Ok(child) => {
                    self.active_player = Some(child);
                    self.status_line = "Playing sample…".into();
                }
                Err(msg) => self.status_line = format!("Player: {msg}"),
            },
            Err(msg) => self.status_line = msg,
        }
        self.preview_busy = false;
        if let Some(queued) = self.queued.take() {
            self.dispatch_preview(queued);
        }
    }

    fn finalize(&mut self) {
        let Some(entry) = self.selected_entry().cloned() else {
            return;
        };
        let region_override = self.region_for(&entry);
        let yaml = emit_scaffold(&entry, region_override.as_deref());
        self.outcome = Some(AppOutcome {
            yaml,
            voice_id: entry.voice_id,
        });
        self.should_quit = true;
    }

    /// Pick the region to stamp into the emitted YAML scaffold for
    /// region-scoped providers. We prefer the user's currently-configured
    /// region (probed at startup) so the output matches their env.
    fn region_for(&self, entry: &VoiceEntry) -> Option<String> {
        match entry.provider {
            ProviderId::Azure => match self.provider_states.get(&ProviderId::Azure) {
                Some(ProviderPreviewState::Ready {
                    selection: crate::tts::provider::ProviderSelection::Azure { region, .. },
                    ..
                }) => Some(region.clone()),
                _ => None,
            },
            ProviderId::Amazon => match self.provider_states.get(&ProviderId::Amazon) {
                Some(ProviderPreviewState::Ready {
                    selection: crate::tts::provider::ProviderSelection::Amazon { region, .. },
                    ..
                }) => Some(region.clone()),
                _ => None,
            },
            _ => None,
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        // Global keys first.
        match key.code {
            KeyCode::Esc => {
                self.should_quit = true;
                return;
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
                return;
            }
            KeyCode::Enter => {
                self.finalize();
                return;
            }
            KeyCode::Up => {
                self.move_up();
                return;
            }
            KeyCode::Down => {
                self.move_down();
                return;
            }
            KeyCode::PageUp => {
                self.page_up(10);
                return;
            }
            KeyCode::PageDown => {
                self.page_down(10);
                return;
            }
            _ => {}
        }

        // Space is ambiguous: we reserve it for preview, so inserting
        // a literal space into the search field requires holding Shift.
        if key.code == KeyCode::Char(' ') && !key.modifiers.contains(KeyModifiers::SHIFT) {
            self.request_preview();
            return;
        }

        // Otherwise forward to the search line and refilter.
        if self.search.handle_event(&Event::Key(key)) {
            self.refilter();
        }
    }
}

pub fn run(
    mut terminal: DefaultTerminal,
    filters: InitialFilters,
    cache: Arc<TtsCache>,
) -> Option<AppOutcome> {
    let mut app = App::new(filters, cache);

    while !app.should_quit {
        terminal.draw(|f| draw(f, &mut app)).ok();

        while let Ok(result) = app.worker.rx.try_recv() {
            app.handle_preview_result(result);
        }
        app.reap_player();

        if event::poll(Duration::from_millis(50)).unwrap_or(false)
            && let Ok(evt) = event::read()
        {
            match evt {
                Event::Key(key) if is_press_or_repeat(&key) => app.handle_key(key),
                Event::Paste(text) => {
                    app.search.insert_str(&text.replace(['\r', '\n'], " "));
                    app.refilter();
                }
                _ => {}
            }
        }
    }

    app.stop_player();
    app.worker.shutdown();
    app.outcome
}

fn is_press_or_repeat(key: &KeyEvent) -> bool {
    matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(rows[0]);

    draw_list_pane(frame, cols[0], app);
    draw_detail_pane(frame, cols[1], app);
    draw_status(frame, rows[1], app);
    draw_footer(frame, rows[2]);
}

fn draw_list_pane(frame: &mut Frame, area: Rect, app: &mut App) {
    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(area);

    let search_title = if app.lang_filter.is_some() || app.provider_filter.is_some() {
        let mut t = String::from(" Search ");
        if let Some(l) = &app.lang_filter {
            t.push_str(&format!("[lang:{l}] "));
        }
        if let Some(p) = app.provider_filter {
            t.push_str(&format!("[provider:{}] ", p.as_str()));
        }
        t
    } else {
        " Search ".to_string()
    };
    let search_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(THEME.border))
        .title(Span::styled(
            search_title,
            Style::default()
                .fg(THEME.header)
                .add_modifier(Modifier::BOLD),
        ));
    let search_para = Paragraph::new(Line::from(vec![
        Span::styled("/ ", Style::default().fg(THEME.dimmed)),
        Span::styled(
            app.search.value().to_string(),
            Style::default().fg(THEME.text),
        ),
    ]))
    .block(search_block);
    frame.render_widget(search_para, inner[0]);

    let items: Vec<ListItem> = app
        .filtered
        .iter()
        .map(|i| render_list_row(&app.entries[*i]))
        .collect();

    let list_title = format!(" Voices ({}) ", app.filtered.len());
    let list_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(THEME.border))
        .title(Span::styled(
            list_title,
            Style::default()
                .fg(THEME.header)
                .add_modifier(Modifier::BOLD),
        ));
    let list = List::new(items)
        .block(list_block)
        .highlight_style(
            Style::default()
                .bg(THEME.highlight_bg)
                .fg(THEME.highlight_fg)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");
    frame.render_stateful_widget(list, inner[1], &mut app.list_state);
}

fn render_list_row(entry: &VoiceEntry) -> ListItem<'static> {
    let provider = format!("{:<7}", entry.provider.as_str());
    let lang = entry
        .languages
        .first()
        .map(String::as_str)
        .unwrap_or(if entry.multilingual { "*" } else { "--" });
    let gender = entry.gender.as_deref().unwrap_or("-");
    let line = Line::from(vec![
        Span::styled(provider, Style::default().fg(THEME.info)),
        Span::raw(" "),
        Span::styled(format!("{:<8}", lang), Style::default().fg(THEME.dimmed)),
        Span::raw(" "),
        Span::styled(format!("{:<6}", gender), Style::default().fg(THEME.dimmed)),
        Span::raw(" "),
        Span::styled(
            entry.voice_id.clone(),
            Style::default().fg(THEME.text).add_modifier(Modifier::BOLD),
        ),
    ]);
    ListItem::new(line)
}

fn draw_detail_pane(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(THEME.border))
        .title(Span::styled(
            " Details ",
            Style::default()
                .fg(THEME.header)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(entry) = app.selected_entry() else {
        let empty = Paragraph::new(Span::styled(
            "No voices match current filters.",
            Style::default().fg(THEME.dimmed),
        ))
        .wrap(Wrap { trim: false });
        frame.render_widget(empty, inner);
        return;
    };

    let state = app.provider_states.get(&entry.provider);
    let status_line = match state {
        Some(ProviderPreviewState::Ready { .. }) => {
            Span::styled("Ready", Style::default().fg(THEME.success))
        }
        Some(ProviderPreviewState::Unavailable { reason }) => Span::styled(
            format!("Unavailable · {reason}"),
            Style::default().fg(THEME.warning),
        ),
        None => Span::styled("Unknown", Style::default().fg(THEME.dimmed)),
    };

    let languages = if entry.multilingual {
        "multilingual".to_string()
    } else if entry.languages.is_empty() {
        "--".into()
    } else {
        entry.languages.join(", ")
    };

    let region = app.region_for(entry);
    let yaml = emit_scaffold(entry, region.as_deref());

    let mut lines: Vec<Line<'static>> = vec![
        kv("Provider", entry.provider.as_str().to_string()),
        kv("Voice", entry.voice_id.clone()),
        kv("Name", entry.display_name.clone()),
        kv("Languages", languages),
        kv(
            "Gender",
            entry.gender.clone().unwrap_or_else(|| "--".into()),
        ),
    ];
    if let Some(m) = &entry.preview_model {
        lines.push(kv("Engine", m.clone()));
    }
    if !entry.tags.is_empty() {
        lines.push(kv("Tags", entry.tags.join(", ")));
    }
    lines.push(Line::from(vec![
        Span::styled("Status: ", Style::default().fg(THEME.dimmed)),
        status_line,
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "YAML scaffold",
        Style::default()
            .fg(THEME.header)
            .add_modifier(Modifier::BOLD),
    )));
    for y_line in yaml.lines() {
        lines.push(Line::from(Span::styled(
            y_line.to_string(),
            Style::default().fg(THEME.text),
        )));
    }

    let para = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(para, inner);
}

fn kv(key: &str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{key}: "), Style::default().fg(THEME.dimmed)),
        Span::styled(value, Style::default().fg(THEME.text)),
    ])
}

fn draw_status(frame: &mut Frame, area: Rect, app: &App) {
    let text = Line::from(Span::styled(
        app.status_line.clone(),
        Style::default().fg(THEME.info),
    ));
    frame.render_widget(Paragraph::new(text), area);
}

fn draw_footer(frame: &mut Frame, area: Rect) {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (i, (key, label)) in [
        ("↑↓", "select"),
        ("type", "search"),
        ("Space", "preview"),
        ("Enter", "copy+emit"),
        ("Esc", "cancel"),
    ]
    .iter()
    .enumerate()
    {
        if i > 0 {
            spans.push(footer_pipe());
        }
        spans.extend(footer_cmd(key, label));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

//! Ratatui event loop for the voice browser.
//!
//! Layout: left pane is a filtered voice list with a visible facet chip
//! row and text search; right pane shows the highlighted voice's details
//! and the YAML scaffold that will be emitted on Enter.

use std::process::Child;
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

use crate::tts::cache::TtsCache;
use crate::tui::line_input::LineInput;
use crate::tui::theme::{THEME, footer_cmd, footer_pipe};

use super::catalog::{
    FacetCatalog, ProviderId, VoiceEntry, VoiceFilters, build_facets, filter, load_snapshot,
};
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

#[derive(Debug, Clone, Copy)]
enum FilterFacet {
    Provider,
    Language,
    Gender,
    Engine,
    Tag,
}

impl FilterFacet {
    fn title(self) -> &'static str {
        match self {
            Self::Provider => "Provider",
            Self::Language => "Language",
            Self::Gender => "Gender",
            Self::Engine => "Engine",
            Self::Tag => "Tags",
        }
    }

    fn key_hint(self) -> &'static str {
        match self {
            Self::Provider => "Ctrl+P",
            Self::Language => "Ctrl+L",
            Self::Gender => "Ctrl+G",
            Self::Engine => "Ctrl+O",
            Self::Tag => "Ctrl+T",
        }
    }

    fn multi_select(self) -> bool {
        matches!(self, Self::Tag)
    }
}

struct FilterOverlay {
    facet: FilterFacet,
    search: LineInput,
    list_state: ListState,
}

impl FilterOverlay {
    fn new(facet: FilterFacet) -> Self {
        let mut list_state = ListState::default();
        list_state.select(Some(0));
        Self {
            facet,
            search: LineInput::default(),
            list_state,
        }
    }

    fn clamp_selection(&mut self, len: usize) {
        let next = if len == 0 {
            None
        } else {
            Some(self.list_state.selected().unwrap_or(0).min(len - 1))
        };
        self.list_state.select(next);
    }
}

#[derive(Debug, Clone)]
enum OverlayAction {
    ClearProvider,
    SetProvider(ProviderId),
    ClearLanguage,
    SetLanguage(String),
    ClearGender,
    SetGender(String),
    ClearEngine,
    SetEngine(String),
    ClearTags,
    ToggleTag(String),
}

#[derive(Debug, Clone)]
struct OverlayRow {
    label: String,
    count: usize,
    selected: bool,
    action: OverlayAction,
}

pub struct App {
    entries: Vec<VoiceEntry>,
    facets: FacetCatalog,
    filtered: Vec<usize>,
    filters: VoiceFilters,
    search: LineInput,
    list_state: ListState,
    overlay: Option<FilterOverlay>,
    show_help: bool,
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
    pub fn new(initial: InitialFilters, cache: Arc<TtsCache>) -> Self {
        let entries = load_snapshot();
        let facets = build_facets(&entries);
        let provider_states = probe_all();
        let worker = spawn_worker();
        let search = LineInput::new(initial.query.unwrap_or_default());
        let filters = VoiceFilters {
            provider: initial.provider,
            language: initial.lang,
            text: search.value().to_string(),
            ..VoiceFilters::default()
        };
        let mut app = Self {
            entries,
            facets,
            filtered: Vec::new(),
            filters,
            search,
            list_state: ListState::default(),
            overlay: None,
            show_help: false,
            provider_states,
            cache,
            worker,
            next_id: 0,
            current_id: 0,
            preview_busy: false,
            queued: None,
            active_player: None,
            status_line:
                "Type to search names · Ctrl+P/L/G/O/T filters · Space=preview · Enter=copy+emit"
                    .into(),
            outcome: None,
            should_quit: false,
        };
        app.refilter();
        app
    }

    fn refilter(&mut self) {
        self.filters.text = self.search.value().to_string();
        self.filtered = filter(&self.entries, &self.filters);
        self.list_state.select(if self.filtered.is_empty() {
            None
        } else {
            Some(0)
        });
        let overlay_state = self
            .overlay
            .as_ref()
            .map(|overlay| (overlay.facet, overlay.search.value().to_string()));
        if let Some((facet, needle)) = overlay_state {
            let len = self.overlay_rows_for(facet, &needle).len();
            if let Some(overlay) = self.overlay.as_mut() {
                overlay.clamp_selection(len);
            }
        }
    }

    fn clear_all_filters(&mut self) {
        self.filters.provider = None;
        self.filters.language = None;
        self.filters.gender = None;
        self.filters.engine = None;
        self.filters.tags.clear();
        self.search.reset();
        self.status_line = "Cleared all filters.".into();
        self.refilter();
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
        self.list_state.select(Some(cur.saturating_sub(1)));
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
            self.status_line = "Queued next preview...".into();
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
        self.status_line = format!("Generating sample for {}...", entry.voice_id);
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
                    self.status_line = "Playing sample...".into();
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

    fn open_overlay(&mut self, facet: FilterFacet) {
        self.stop_player();
        self.overlay = Some(FilterOverlay::new(facet));
        let len = self.overlay_rows_for(facet, "").len();
        if let Some(overlay) = self.overlay.as_mut() {
            overlay.clamp_selection(len);
        }
        self.status_line = format!("{} filter", facet.title());
    }

    fn overlay_rows(&self) -> Vec<OverlayRow> {
        let Some(overlay) = &self.overlay else {
            return Vec::new();
        };
        self.overlay_rows_for(overlay.facet, overlay.search.value())
    }

    fn overlay_rows_for(&self, facet: FilterFacet, needle: &str) -> Vec<OverlayRow> {
        let needle = needle.trim().to_ascii_lowercase();
        let include =
            |label: &str| needle.is_empty() || label.to_ascii_lowercase().contains(&needle);
        let include_clear_row = needle.is_empty();
        let mut rows = Vec::new();
        match facet {
            FilterFacet::Provider => {
                if include_clear_row {
                    rows.push(OverlayRow {
                        label: "Any provider".into(),
                        count: self.entries.len(),
                        selected: self.filters.provider.is_none(),
                        action: OverlayAction::ClearProvider,
                    });
                }
                for (provider, count) in &self.facets.providers {
                    let label = provider.as_str().to_string();
                    if include(&label) {
                        rows.push(OverlayRow {
                            label,
                            count: *count,
                            selected: self.filters.provider == Some(*provider),
                            action: OverlayAction::SetProvider(*provider),
                        });
                    }
                }
            }
            FilterFacet::Language => {
                if include_clear_row {
                    rows.push(OverlayRow {
                        label: "Any language".into(),
                        count: self.entries.len(),
                        selected: self.filters.language.is_none(),
                        action: OverlayAction::ClearLanguage,
                    });
                }
                for (language, count) in &self.facets.languages {
                    if include(language) {
                        rows.push(OverlayRow {
                            label: language.clone(),
                            count: *count,
                            selected: self.filters.language.as_deref() == Some(language.as_str()),
                            action: OverlayAction::SetLanguage(language.clone()),
                        });
                    }
                }
            }
            FilterFacet::Gender => {
                if include_clear_row {
                    rows.push(OverlayRow {
                        label: "Any gender".into(),
                        count: self.entries.len(),
                        selected: self.filters.gender.is_none(),
                        action: OverlayAction::ClearGender,
                    });
                }
                for (gender, count) in &self.facets.genders {
                    if include(gender) {
                        rows.push(OverlayRow {
                            label: gender.clone(),
                            count: *count,
                            selected: self.filters.gender.as_deref() == Some(gender.as_str()),
                            action: OverlayAction::SetGender(gender.clone()),
                        });
                    }
                }
            }
            FilterFacet::Engine => {
                if include_clear_row {
                    rows.push(OverlayRow {
                        label: "Any engine".into(),
                        count: self.entries.len(),
                        selected: self.filters.engine.is_none(),
                        action: OverlayAction::ClearEngine,
                    });
                }
                for (engine, count) in &self.facets.engines {
                    if include(engine) {
                        rows.push(OverlayRow {
                            label: engine.clone(),
                            count: *count,
                            selected: self.filters.engine.as_deref() == Some(engine.as_str()),
                            action: OverlayAction::SetEngine(engine.clone()),
                        });
                    }
                }
            }
            FilterFacet::Tag => {
                if include_clear_row {
                    rows.push(OverlayRow {
                        label: "Clear all tags".into(),
                        count: self.filters.tags.len(),
                        selected: self.filters.tags.is_empty(),
                        action: OverlayAction::ClearTags,
                    });
                }
                for (tag, count) in &self.facets.tags {
                    if include(tag) {
                        rows.push(OverlayRow {
                            label: tag.clone(),
                            count: *count,
                            selected: self.filters.tags.iter().any(|t| t == tag),
                            action: OverlayAction::ToggleTag(tag.clone()),
                        });
                    }
                }
            }
        }
        rows
    }

    fn apply_overlay_action(&mut self, action: OverlayAction, close_after: bool) {
        match action {
            OverlayAction::ClearProvider => self.filters.provider = None,
            OverlayAction::SetProvider(provider) => self.filters.provider = Some(provider),
            OverlayAction::ClearLanguage => self.filters.language = None,
            OverlayAction::SetLanguage(language) => self.filters.language = Some(language),
            OverlayAction::ClearGender => self.filters.gender = None,
            OverlayAction::SetGender(gender) => self.filters.gender = Some(gender),
            OverlayAction::ClearEngine => self.filters.engine = None,
            OverlayAction::SetEngine(engine) => self.filters.engine = Some(engine),
            OverlayAction::ClearTags => self.filters.tags.clear(),
            OverlayAction::ToggleTag(tag) => {
                if let Some(idx) = self
                    .filters
                    .tags
                    .iter()
                    .position(|existing| existing == &tag)
                {
                    self.filters.tags.remove(idx);
                } else {
                    self.filters.tags.push(tag);
                    self.filters.tags.sort();
                }
            }
        }
        self.refilter();
        if close_after {
            self.overlay = None;
        }
    }

    fn handle_overlay_key(&mut self, key: KeyEvent) {
        let rows = self.overlay_rows();
        let selected = self
            .overlay
            .as_ref()
            .and_then(|overlay| overlay.list_state.selected())
            .unwrap_or(0);
        match key.code {
            KeyCode::Esc => {
                self.overlay = None;
            }
            KeyCode::Up => {
                if let Some(overlay) = self.overlay.as_mut() {
                    let next = selected.saturating_sub(1);
                    overlay.list_state.select(Some(next));
                }
            }
            KeyCode::Down => {
                if let Some(overlay) = self.overlay.as_mut() {
                    let max = rows.len().saturating_sub(1);
                    overlay.list_state.select(Some((selected + 1).min(max)));
                }
            }
            KeyCode::PageUp => {
                if let Some(overlay) = self.overlay.as_mut() {
                    overlay.list_state.select(Some(selected.saturating_sub(10)));
                }
            }
            KeyCode::PageDown => {
                if let Some(overlay) = self.overlay.as_mut() {
                    let max = rows.len().saturating_sub(1);
                    overlay.list_state.select(Some((selected + 10).min(max)));
                }
            }
            KeyCode::Enter => {
                if let Some(row) = rows.get(selected) {
                    self.apply_overlay_action(row.action.clone(), true);
                }
            }
            KeyCode::Char(' ') => {
                let multi = self
                    .overlay
                    .as_ref()
                    .map(|overlay| overlay.facet.multi_select())
                    .unwrap_or(false);
                if multi && let Some(row) = rows.get(selected) {
                    self.apply_overlay_action(row.action.clone(), false);
                }
            }
            _ => {
                if let Some(overlay) = self.overlay.as_mut()
                    && overlay.search.handle_event(&Event::Key(key))
                {
                    let facet = overlay.facet;
                    let needle = overlay.search.value().to_string();
                    let _ = overlay;
                    let len = self.overlay_rows_for(facet, &needle).len();
                    if let Some(overlay) = self.overlay.as_mut() {
                        overlay
                            .list_state
                            .select(if len == 0 { None } else { Some(0) });
                    }
                }
            }
        }
    }

    fn handle_paste(&mut self, text: String) {
        self.stop_player();
        let cleaned = text.replace(['\r', '\n'], " ");
        if let Some(overlay) = self.overlay.as_mut() {
            overlay.search.insert_str(&cleaned);
            let facet = overlay.facet;
            let needle = overlay.search.value().to_string();
            let _ = overlay;
            let len = self.overlay_rows_for(facet, &needle).len();
            if let Some(overlay) = self.overlay.as_mut() {
                overlay
                    .list_state
                    .select(if len == 0 { None } else { Some(0) });
            }
            return;
        }
        self.search.insert_str(&cleaned);
        self.refilter();
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if self.show_help {
            self.show_help = false;
            return;
        }
        if self.overlay.is_some() {
            self.handle_overlay_key(key);
            return;
        }

        let keeps_playing = matches!(
            key.code,
            KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown
        );
        if !keeps_playing
            && (key.code != KeyCode::Char(' ') || key.modifiers.contains(KeyModifiers::SHIFT))
        {
            self.stop_player();
        }

        if key.code == KeyCode::Char('?') {
            self.show_help = true;
            return;
        }

        if let KeyCode::Char(c) = key.code
            && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            match c.to_ascii_lowercase() {
                'p' => {
                    self.open_overlay(FilterFacet::Provider);
                    return;
                }
                'l' => {
                    self.open_overlay(FilterFacet::Language);
                    return;
                }
                'g' => {
                    self.open_overlay(FilterFacet::Gender);
                    return;
                }
                'o' => {
                    self.open_overlay(FilterFacet::Engine);
                    return;
                }
                't' => {
                    self.open_overlay(FilterFacet::Tag);
                    return;
                }
                _ => {}
            }
        }

        match key.code {
            KeyCode::Esc => {
                self.should_quit = true;
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.clear_all_filters();
            }
            KeyCode::Enter => {
                self.finalize();
            }
            KeyCode::Up => self.move_up(),
            KeyCode::Down => self.move_down(),
            KeyCode::PageUp => self.page_up(10),
            KeyCode::PageDown => self.page_down(10),
            KeyCode::Char(' ') if !key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.request_preview();
            }
            _ => {
                if self.search.handle_event(&Event::Key(key)) {
                    self.refilter();
                }
            }
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
                Event::Paste(text) => app.handle_paste(text),
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

    if app.show_help {
        draw_help_overlay(frame);
    }
    if app.overlay.is_some() {
        draw_filter_overlay(frame, app);
    }
}

fn draw_list_pane(frame: &mut Frame, area: Rect, app: &mut App) {
    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Min(0),
        ])
        .split(area);

    let chips = Paragraph::new(render_chip_row(app));
    frame.render_widget(chips, inner[0]);

    let search_value = if app.search.value().is_empty() {
        Span::styled(
            "voice id or display name",
            Style::default().fg(THEME.dimmed),
        )
    } else {
        Span::styled(
            app.search.value().to_string(),
            Style::default().fg(THEME.text),
        )
    };
    let search_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(THEME.border))
        .title(Span::styled(
            " Text Search ",
            Style::default()
                .fg(THEME.header)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled("Ctrl+P/L/G/O/T", Style::default().fg(THEME.dimmed)),
            Span::styled(" filters ", Style::default().fg(THEME.help_muted)),
            Span::styled("Ctrl+R", Style::default().fg(THEME.dimmed)),
            Span::styled(" clear ", Style::default().fg(THEME.help_muted)),
            Span::styled("?", Style::default().fg(THEME.dimmed)),
            Span::styled(" help ", Style::default().fg(THEME.help_muted)),
        ]));
    let search_para = Paragraph::new(Line::from(vec![
        Span::styled("/ ", Style::default().fg(THEME.dimmed)),
        search_value,
    ]))
    .block(search_block);
    frame.render_widget(search_para, inner[1]);

    let items: Vec<ListItem> = app
        .filtered
        .iter()
        .map(|i| render_list_row(&app.entries[*i]))
        .collect();
    let list_title = if app.filters.active_count() == 0 {
        format!(" Voices ({}) ", app.filtered.len())
    } else {
        format!(
            " Voices ({}, {} active) ",
            app.filtered.len(),
            app.filters.active_count()
        )
    };
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(THEME.border))
                .title(Span::styled(
                    list_title,
                    Style::default()
                        .fg(THEME.header)
                        .add_modifier(Modifier::BOLD),
                )),
        )
        .highlight_style(
            Style::default()
                .bg(THEME.highlight_bg)
                .fg(THEME.highlight_fg)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");
    frame.render_stateful_widget(list, inner[2], &mut app.list_state);
}

fn render_chip_row(app: &App) -> Line<'static> {
    let mut spans = Vec::new();
    let chips = [
        (
            "Provider",
            app.filters
                .provider
                .map(|provider| provider.as_str().to_string()),
            THEME.info,
        ),
        ("Lang", app.filters.language.clone(), THEME.success),
        ("Gender", app.filters.gender.clone(), THEME.warning),
        ("Engine", app.filters.engine.clone(), THEME.info),
        (
            "Tags",
            if app.filters.tags.is_empty() {
                None
            } else {
                Some(app.filters.tags.join(","))
            },
            THEME.success,
        ),
    ];
    for (idx, (label, value, color)) in chips.into_iter().enumerate() {
        if idx > 0 {
            spans.push(Span::styled("  ", Style::default()));
        }
        spans.push(Span::styled(
            format!("{label}: "),
            Style::default().fg(THEME.dimmed),
        ));
        match value {
            Some(value) => spans.push(Span::styled(
                value,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            )),
            None => spans.push(Span::styled("any", Style::default().fg(THEME.dimmed))),
        }
    }
    Line::from(spans)
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
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            app.status_line.clone(),
            Style::default().fg(THEME.info),
        ))),
        area,
    );
}

fn draw_footer(frame: &mut Frame, area: Rect) {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (idx, (key, label)) in [
        ("↑↓", "select"),
        ("type", "text"),
        ("Ctrl+P/L/G/O/T", "filter"),
        ("Space", "preview"),
        ("Enter", "copy+emit"),
        ("Esc", "cancel"),
    ]
    .iter()
    .enumerate()
    {
        if idx > 0 {
            spans.push(footer_pipe());
        }
        spans.extend(footer_cmd(key, label));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_help_overlay(frame: &mut Frame) {
    let area = centered_rect(frame.area(), 62, 12);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(THEME.help_border))
        .title(Span::styled(
            " Help ",
            Style::default()
                .fg(THEME.header)
                .add_modifier(Modifier::BOLD),
        ));
    let lines = vec![
        Line::from(vec![
            Span::styled("Type", Style::default().fg(THEME.dimmed)),
            Span::styled(
                " to search voice id and display name",
                Style::default().fg(THEME.text),
            ),
        ]),
        Line::from(vec![
            Span::styled("Ctrl+P", Style::default().fg(THEME.dimmed)),
            Span::styled(" provider filter", Style::default().fg(THEME.text)),
        ]),
        Line::from(vec![
            Span::styled("Ctrl+L", Style::default().fg(THEME.dimmed)),
            Span::styled(" language filter", Style::default().fg(THEME.text)),
        ]),
        Line::from(vec![
            Span::styled("Ctrl+G", Style::default().fg(THEME.dimmed)),
            Span::styled(" gender filter", Style::default().fg(THEME.text)),
        ]),
        Line::from(vec![
            Span::styled("Ctrl+O", Style::default().fg(THEME.dimmed)),
            Span::styled(" engine filter", Style::default().fg(THEME.text)),
        ]),
        Line::from(vec![
            Span::styled("Ctrl+T", Style::default().fg(THEME.dimmed)),
            Span::styled(
                " tag filter (Space toggles)",
                Style::default().fg(THEME.text),
            ),
        ]),
        Line::from(vec![
            Span::styled("Ctrl+R", Style::default().fg(THEME.dimmed)),
            Span::styled(" clear all filters", Style::default().fg(THEME.text)),
        ]),
        Line::from(vec![
            Span::styled("Any key", Style::default().fg(THEME.dimmed)),
            Span::styled(" closes this help", Style::default().fg(THEME.text)),
        ]),
    ];
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_filter_overlay(frame: &mut Frame, app: &mut App) {
    let Some((facet, needle)) = app
        .overlay
        .as_ref()
        .map(|overlay| (overlay.facet, overlay.search.value().to_string()))
    else {
        return;
    };
    let rows = app.overlay_rows_for(facet, &needle);
    let Some(overlay) = &mut app.overlay else {
        return;
    };
    overlay.clamp_selection(rows.len());

    let area = centered_rect(
        frame.area(),
        if matches!(facet, FilterFacet::Language | FilterFacet::Tag) {
            64
        } else {
            52
        },
        18,
    );
    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(area);

    let search_value = if overlay.search.value().is_empty() {
        Span::styled("type to narrow", Style::default().fg(THEME.dimmed))
    } else {
        Span::styled(
            overlay.search.value().to_string(),
            Style::default().fg(THEME.text),
        )
    };
    let search_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(THEME.help_border))
        .title(Span::styled(
            format!(" {} ", overlay.facet.title()),
            Style::default()
                .fg(THEME.header)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled("Enter", Style::default().fg(THEME.dimmed)),
            Span::styled(" apply ", Style::default().fg(THEME.help_muted)),
            Span::styled("Esc", Style::default().fg(THEME.dimmed)),
            Span::styled(" close ", Style::default().fg(THEME.help_muted)),
            Span::styled("↑↓", Style::default().fg(THEME.dimmed)),
            Span::styled(" move ", Style::default().fg(THEME.help_muted)),
            Span::styled(
                if facet.multi_select() {
                    "Space"
                } else {
                    facet.key_hint()
                },
                Style::default().fg(THEME.dimmed),
            ),
            Span::styled(
                if facet.multi_select() {
                    " toggle "
                } else {
                    " facet "
                },
                Style::default().fg(THEME.help_muted),
            ),
        ]));
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("/ ", Style::default().fg(THEME.dimmed)),
            search_value,
        ]))
        .block(search_block),
        parts[0],
    );

    let items: Vec<ListItem> = rows
        .into_iter()
        .map(|row| {
            let marker = if facet.multi_select() && !matches!(row.action, OverlayAction::ClearTags)
            {
                if row.selected { "[x] " } else { "[ ] " }
            } else if row.selected {
                "* "
            } else {
                "  "
            };
            let style = if row.selected {
                Style::default().fg(THEME.success)
            } else {
                Style::default().fg(THEME.text)
            };
            ListItem::new(Line::from(vec![
                Span::styled(marker, Style::default().fg(THEME.dimmed)),
                Span::styled(row.label, style),
                Span::raw(" "),
                Span::styled(
                    format!("({})", row.count),
                    Style::default().fg(THEME.dimmed),
                ),
            ]))
        })
        .collect();
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(THEME.help_border)),
        )
        .highlight_style(
            Style::default()
                .bg(THEME.highlight_bg)
                .fg(THEME.highlight_fg)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");
    frame.render_stateful_widget(list, parts[1], &mut overlay.list_state);
}

fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    Rect::new(
        area.x + area.width.saturating_sub(width.min(area.width)) / 2,
        area.y + area.height.saturating_sub(height.min(area.height)) / 2,
        width.min(area.width),
        height.min(area.height),
    )
}

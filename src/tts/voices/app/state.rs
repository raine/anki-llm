use std::collections::HashMap;
use std::process::Child;
use std::sync::Arc;

use ratatui::widgets::ListState;

use crate::tts::cache::TtsCache;
use crate::tui::line_input::LineInput;

use crate::tts::voices::catalog::{
    FacetCatalog, ProviderId, VoiceEntry, VoiceFilters, build_facets, filter,
};
use crate::tts::voices::credentials::ProviderPreviewState;
use crate::tts::voices::player;
use crate::tts::voices::preview::{PreviewHandle, RequestId};

pub struct InitialFilters {
    pub lang: Option<String>,
    pub provider: Option<ProviderId>,
    pub query: Option<String>,
}

pub(super) struct AppDependencies {
    pub entries: Vec<VoiceEntry>,
    pub provider_states: HashMap<ProviderId, ProviderPreviewState>,
    pub cache: Arc<TtsCache>,
    pub worker: PreviewHandle,
}

pub(super) struct Toast {
    pub message: String,
    pub tick: u64,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum FilterFacet {
    Provider,
    Language,
    Gender,
    Engine,
    Tag,
}

impl FilterFacet {
    pub(super) fn title(self) -> &'static str {
        match self {
            Self::Provider => "Provider",
            Self::Language => "Language",
            Self::Gender => "Gender",
            Self::Engine => "Engine",
            Self::Tag => "Tags",
        }
    }

    pub(super) fn key_hint(self) -> &'static str {
        match self {
            Self::Provider => "Ctrl+P",
            Self::Language => "Ctrl+L",
            Self::Gender => "Ctrl+G",
            Self::Engine => "Ctrl+O",
            Self::Tag => "Ctrl+T",
        }
    }

    pub(super) fn multi_select(self) -> bool {
        matches!(self, Self::Tag)
    }
}

pub(super) struct FilterOverlay {
    pub facet: FilterFacet,
    pub search: LineInput,
    pub selected: Option<usize>,
}

impl FilterOverlay {
    pub(super) fn new(facet: FilterFacet) -> Self {
        Self {
            facet,
            search: LineInput::default(),
            selected: Some(0),
        }
    }

    pub(super) fn reset_selection(&mut self, len: usize) {
        self.selected = first_selection(len);
    }

    pub(super) fn clamp_selection(&mut self, len: usize) {
        self.selected = clamp_selection(self.selected, len);
    }
}

#[derive(Default)]
pub(super) struct ViewState {
    pub list_state: ListState,
    pub overlay_list_state: ListState,
}

impl ViewState {
    pub(super) fn sync_from(&mut self, app: &App) {
        self.list_state.select(app.selected);
        self.overlay_list_state
            .select(app.overlay.as_ref().and_then(|overlay| overlay.selected));
    }
}

fn first_selection(len: usize) -> Option<usize> {
    if len == 0 { None } else { Some(0) }
}

fn clamp_selection(selected: Option<usize>, len: usize) -> Option<usize> {
    if len == 0 {
        None
    } else {
        Some(selected.unwrap_or(0).min(len - 1))
    }
}

#[derive(Debug, Clone)]
pub(super) enum OverlayAction {
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
pub(super) struct OverlayRow {
    pub label: String,
    pub count: usize,
    pub selected: bool,
    pub action: OverlayAction,
}

pub(super) struct App {
    pub entries: Vec<VoiceEntry>,
    pub facets: FacetCatalog,
    pub filtered: Vec<usize>,
    pub filters: VoiceFilters,
    pub search: LineInput,
    pub selected: Option<usize>,
    pub overlay: Option<FilterOverlay>,
    pub show_help: bool,
    pub provider_states: HashMap<ProviderId, ProviderPreviewState>,
    pub cache: Arc<TtsCache>,
    pub worker: PreviewHandle,
    pub next_id: RequestId,
    pub current_id: RequestId,
    pub preview_busy: bool,
    pub queued: Option<usize>,
    pub active_player: Option<Child>,
    pub status_line: String,
    pub toast: Option<Toast>,
    pub tick: u64,
    pub should_quit: bool,
}

impl App {
    pub(super) fn new(initial: InitialFilters, deps: AppDependencies) -> Self {
        let AppDependencies {
            entries,
            provider_states,
            cache,
            worker,
        } = deps;
        let facets = build_facets(&entries);
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
            selected: None,
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
                "Type to search names · Ctrl+P/L/G/O/T filters · Space=preview · Enter=copy yaml"
                    .into(),
            toast: None,
            tick: 0,
            should_quit: false,
        };
        app.refilter();
        app
    }

    pub(super) fn refilter(&mut self) {
        self.filters.text = self.search.value().to_string();
        self.filtered = filter(&self.entries, &self.filters);
        self.selected = first_selection(self.filtered.len());
        let overlay_state = self
            .overlay
            .as_ref()
            .map(|overlay| (overlay.facet, overlay.search.value().to_string()));
        if let Some((facet, needle)) = overlay_state {
            self.clamp_overlay_selection(facet, &needle);
        }
    }

    pub(super) fn clear_all_filters(&mut self) {
        self.filters.provider = None;
        self.filters.language = None;
        self.filters.gender = None;
        self.filters.engine = None;
        self.filters.tags.clear();
        self.search.reset();
        self.status_line = "Cleared all filters.".into();
        self.refilter();
    }

    pub(super) fn selected_index(&self) -> Option<usize> {
        self.selected.and_then(|i| self.filtered.get(i).copied())
    }

    pub(super) fn selected_entry(&self) -> Option<&VoiceEntry> {
        self.selected_index().map(|i| &self.entries[i])
    }

    pub(super) fn move_up(&mut self) {
        if self.filtered.is_empty() {
            self.selected = None;
            return;
        }
        let max = self.filtered.len().saturating_sub(1);
        let cur = self.selected.unwrap_or(0).min(max);
        self.selected = Some(cur.saturating_sub(1));
    }

    pub(super) fn move_down(&mut self) {
        if self.filtered.is_empty() {
            self.selected = None;
            return;
        }
        let cur = self.selected.unwrap_or(0);
        let next = (cur + 1).min(self.filtered.len().saturating_sub(1));
        self.selected = Some(next);
    }

    pub(super) fn page_up(&mut self, rows: usize) {
        if self.filtered.is_empty() {
            self.selected = None;
            return;
        }
        let max = self.filtered.len().saturating_sub(1);
        let cur = self.selected.unwrap_or(0).min(max);
        self.selected = Some(cur.saturating_sub(rows));
    }

    pub(super) fn page_down(&mut self, rows: usize) {
        if self.filtered.is_empty() {
            self.selected = None;
            return;
        }
        let cur = self.selected.unwrap_or(0);
        let max = self.filtered.len().saturating_sub(1);
        self.selected = Some((cur + rows).min(max));
    }

    pub(super) fn stop_player(&mut self) {
        if let Some(child) = self.active_player.take() {
            player::stop(child);
        }
    }

    pub(super) fn reap_player(&mut self) {
        if let Some(child) = self.active_player.as_mut()
            && let Ok(Some(_)) = child.try_wait()
        {
            self.active_player = None;
        }
    }

    pub(super) fn region_for(&self, entry: &VoiceEntry) -> Option<String> {
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

    pub(super) fn overlay_rows(&self) -> Vec<OverlayRow> {
        let Some(overlay) = &self.overlay else {
            return Vec::new();
        };
        self.overlay_rows_for(overlay.facet, overlay.search.value())
    }

    pub(super) fn reset_overlay_selection(&mut self, facet: FilterFacet, needle: &str) {
        let len = self.overlay_rows_for(facet, needle).len();
        if let Some(overlay) = self.overlay.as_mut() {
            overlay.reset_selection(len);
        }
    }

    pub(super) fn clamp_overlay_selection(&mut self, facet: FilterFacet, needle: &str) {
        let len = self.overlay_rows_for(facet, needle).len();
        if let Some(overlay) = self.overlay.as_mut() {
            overlay.clamp_selection(len);
        }
    }

    pub(super) fn overlay_selected(&self) -> Option<usize> {
        self.overlay.as_ref().and_then(|overlay| overlay.selected)
    }

    pub(super) fn select_overlay(&mut self, selected: Option<usize>) {
        if let Some(overlay) = self.overlay.as_mut() {
            overlay.selected = selected;
        }
    }

    pub(super) fn overlay_rows_for(&self, facet: FilterFacet, needle: &str) -> Vec<OverlayRow> {
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

    pub(super) fn apply_overlay_action(&mut self, action: OverlayAction, close_after: bool) {
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
}

#[cfg(test)]
mod tests {
    use std::process::{Child, Command};
    use std::sync::mpsc::{self, Receiver};
    use std::thread;
    use std::time::{Duration, Instant};

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::tts::voices::preview::{PreviewRequest, PreviewResult};

    use super::super::draw::draw;
    use super::*;

    struct PreviewHarness {
        handle: PreviewHandle,
        req_rx: Receiver<Option<PreviewRequest>>,
    }

    fn preview_harness() -> PreviewHarness {
        let (req_tx, req_rx) = mpsc::channel();
        let (res_tx, res_rx) = mpsc::channel();
        drop(res_tx);
        PreviewHarness {
            handle: PreviewHandle::from_channels(req_tx, res_rx),
            req_rx,
        }
    }

    fn test_worker() -> PreviewHandle {
        preview_harness().handle
    }

    fn voice(
        provider: ProviderId,
        voice_id: &str,
        display_name: &str,
        language: &str,
    ) -> VoiceEntry {
        rich_voice(
            provider,
            voice_id,
            display_name,
            &[language],
            false,
            None,
            None,
            &[],
        )
    }

    fn rich_voice(
        provider: ProviderId,
        voice_id: &str,
        display_name: &str,
        languages: &[&str],
        multilingual: bool,
        gender: Option<&str>,
        preview_model: Option<&str>,
        tags: &[&str],
    ) -> VoiceEntry {
        VoiceEntry {
            provider,
            voice_id: voice_id.into(),
            display_name: display_name.into(),
            languages: languages
                .iter()
                .map(|language| (*language).into())
                .collect(),
            multilingual,
            gender: gender.map(str::to_string),
            preview_model: preview_model.map(str::to_string),
            tags: tags.iter().map(|tag| (*tag).into()).collect(),
        }
    }

    fn test_app(entries: Vec<VoiceEntry>) -> App {
        let harness = preview_harness();
        test_app_with_worker(entries, harness.handle)
    }

    fn test_app_with_worker(entries: Vec<VoiceEntry>, worker: PreviewHandle) -> App {
        let tmp = tempfile::tempdir().unwrap();
        let cache = Arc::new(TtsCache::new(tmp.path().to_path_buf()).unwrap());
        App::new(
            InitialFilters {
                lang: None,
                provider: None,
                query: None,
            },
            AppDependencies {
                entries,
                provider_states: HashMap::new(),
                cache,
                worker,
            },
        )
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn app_snapshot(app: &App) -> String {
        let overlay = app.overlay.as_ref().map(|overlay| {
            (
                overlay.facet.title(),
                overlay.search.value().to_string(),
                overlay.selected,
            )
        });
        format!(
            "filtered={:?};filters={:?};search={:?};selected={:?};overlay={:?};help={};next={};current={};busy={};queued={:?};status={:?};toast={:?};tick={};quit={}",
            app.filtered,
            app.filters,
            app.search.value(),
            app.selected,
            overlay,
            app.show_help,
            app.next_id,
            app.current_id,
            app.preview_busy,
            app.queued,
            app.status_line,
            app.toast.as_ref().map(|toast| (&toast.message, toast.tick)),
            app.tick,
            app.should_quit,
        )
    }

    fn spawn_shell(script: &str) -> Child {
        Command::new("sh").arg("-c").arg(script).spawn().unwrap()
    }

    fn wait_for_exit(child: &mut Child) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if child.try_wait().unwrap().is_some() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("child did not exit before deadline");
    }

    #[test]
    fn constructs_app_from_explicit_dependencies_without_side_effects() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = Arc::new(TtsCache::new(tmp.path().to_path_buf()).unwrap());
        let app = App::new(
            InitialFilters {
                lang: Some("ja".into()),
                provider: Some(ProviderId::Azure),
                query: Some("nanami".into()),
            },
            AppDependencies {
                entries: vec![
                    voice(ProviderId::Azure, "ja-JP-NanamiNeural", "Nanami", "ja-JP"),
                    voice(ProviderId::Google, "en-US-Studio-O", "Studio O", "en-US"),
                ],
                provider_states: HashMap::new(),
                cache,
                worker: test_worker(),
            },
        );

        assert_eq!(app.search.value(), "nanami");
        assert_eq!(app.filters.text, "nanami");
        assert_eq!(app.filters.language.as_deref(), Some("ja"));
        assert_eq!(app.filters.provider, Some(ProviderId::Azure));
        assert_eq!(app.filtered, vec![0]);
        assert_eq!(app.selected, Some(0));
        assert!(!app.preview_busy);
        assert!(
            app.selected_entry()
                .is_some_and(|entry| entry.voice_id == "ja-JP-NanamiNeural")
        );
    }

    #[test]
    fn clamps_overlay_selection_when_rows_change() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = Arc::new(TtsCache::new(tmp.path().to_path_buf()).unwrap());
        let mut app = App::new(
            InitialFilters {
                lang: None,
                provider: None,
                query: None,
            },
            AppDependencies {
                entries: vec![
                    voice(ProviderId::Azure, "ja-JP-NanamiNeural", "Nanami", "ja-JP"),
                    voice(ProviderId::Google, "en-US-Studio-O", "Studio O", "en-US"),
                ],
                provider_states: HashMap::new(),
                cache,
                worker: test_worker(),
            },
        );

        app.overlay = Some(FilterOverlay::new(FilterFacet::Provider));
        app.select_overlay(Some(2));
        app.clamp_overlay_selection(FilterFacet::Provider, "azure");
        assert_eq!(app.overlay_selected(), Some(0));

        app.reset_overlay_selection(FilterFacet::Provider, "no matches");
        assert_eq!(app.overlay_selected(), None);
    }

    #[test]
    fn keeps_empty_overlay_selection_unselected() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = Arc::new(TtsCache::new(tmp.path().to_path_buf()).unwrap());
        let mut app = App::new(
            InitialFilters {
                lang: None,
                provider: None,
                query: None,
            },
            AppDependencies {
                entries: vec![voice(
                    ProviderId::Azure,
                    "ja-JP-NanamiNeural",
                    "Nanami",
                    "ja-JP",
                )],
                provider_states: HashMap::new(),
                cache,
                worker: test_worker(),
            },
        );

        app.overlay = Some(FilterOverlay::new(FilterFacet::Provider));
        if let Some(overlay) = app.overlay.as_mut() {
            overlay.search.insert_str("no matches");
        }
        app.reset_overlay_selection(FilterFacet::Provider, "no matches");

        for code in [
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::PageUp,
            KeyCode::PageDown,
        ] {
            app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
            assert_eq!(app.overlay_selected(), None);
        }
    }

    #[test]
    fn filter_application_updates_results_and_selection() {
        let mut app = test_app(vec![
            rich_voice(
                ProviderId::Azure,
                "ja-JP-NanamiNeural",
                "Nanami",
                &["ja-JP"],
                false,
                Some("Female"),
                Some("neural"),
                &["neural", "friendly"],
            ),
            rich_voice(
                ProviderId::Google,
                "en-US-Studio-O",
                "Studio O",
                &["en-US"],
                false,
                Some("Female"),
                None,
                &["studio"],
            ),
            rich_voice(
                ProviderId::Openai,
                "alloy",
                "Alloy",
                &[],
                true,
                None,
                None,
                &["multilingual"],
            ),
        ]);

        app.handle_key(key(KeyCode::Char('n')));
        app.handle_key(key(KeyCode::Char('a')));
        assert_eq!(app.filters.text, "na");
        assert_eq!(app.filtered, vec![0]);
        assert_eq!(app.selected, Some(0));

        app.clear_all_filters();
        app.filters.language = Some("ja".into());
        app.refilter();
        assert_eq!(
            app.filtered
                .iter()
                .map(|idx| app.entries[*idx].voice_id.as_str())
                .collect::<Vec<_>>(),
            vec!["ja-JP-NanamiNeural", "alloy"]
        );

        app.filters.provider = Some(ProviderId::Azure);
        app.filters.tags = vec!["friendly".into(), "neural".into()];
        app.refilter();
        assert_eq!(app.filtered, vec![0]);
        assert_eq!(app.selected, Some(0));

        app.search.reset();
        app.search.insert_str("missing");
        app.refilter();
        assert!(app.filtered.is_empty());
        assert_eq!(app.selected, None);

        app.handle_key(ctrl('r'));
        assert_eq!(app.filters.active_count(), 0);
        assert_eq!(app.filtered, vec![0, 1, 2]);
        assert_eq!(app.selected, Some(0));
    }

    #[test]
    fn overlay_actions_update_filters_and_close_semantics() {
        let mut app = test_app(vec![
            rich_voice(
                ProviderId::Azure,
                "ja-JP-NanamiNeural",
                "Nanami",
                &["ja-JP"],
                false,
                Some("Female"),
                Some("neural"),
                &["friendly", "neural"],
            ),
            rich_voice(
                ProviderId::Google,
                "en-US-Studio-O",
                "Studio O",
                &["en-US"],
                false,
                Some("Female"),
                None,
                &["studio"],
            ),
        ]);

        app.handle_key(ctrl('p'));
        assert!(app.overlay.is_some());
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.filters.provider, Some(ProviderId::Azure));
        assert!(app.overlay.is_none());
        assert_eq!(app.filtered, vec![0]);

        app.handle_key(ctrl('t'));
        let rows = app.overlay_rows();
        let neural = rows.iter().position(|row| row.label == "neural").unwrap();
        app.select_overlay(Some(neural));
        app.handle_key(key(KeyCode::Char(' ')));
        assert!(app.overlay.is_some());
        assert_eq!(app.filters.tags, vec!["neural"]);
        assert_eq!(app.filtered, vec![0]);

        let friendly = app
            .overlay_rows()
            .iter()
            .position(|row| row.label == "friendly")
            .unwrap();
        app.select_overlay(Some(friendly));
        app.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(app.filters.tags, vec!["friendly", "neural"]);

        app.handle_key(key(KeyCode::Enter));
        assert!(app.overlay.is_none());
        assert_eq!(app.filters.tags, vec!["neural"]);

        app.apply_overlay_action(OverlayAction::ClearTags, false);
        assert!(app.filters.tags.is_empty());
        assert_eq!(app.filtered, vec![0]);
    }

    #[test]
    fn stale_main_selection_clamps_during_update_not_draw() {
        let mut app = test_app(vec![
            voice(ProviderId::Azure, "ja-JP-NanamiNeural", "Nanami", "ja-JP"),
            voice(ProviderId::Google, "en-US-Studio-O", "Studio O", "en-US"),
        ]);

        app.selected = Some(99);
        app.move_down();
        assert_eq!(app.selected, Some(1));

        app.selected = Some(99);
        app.page_down(10);
        assert_eq!(app.selected, Some(1));

        app.selected = Some(99);
        app.move_up();
        assert_eq!(app.selected, Some(0));

        app.selected = Some(99);
        app.page_up(10);
        assert_eq!(app.selected, Some(0));
    }

    #[test]
    fn preview_queue_dispatches_after_stale_and_current_results() {
        let harness = preview_harness();
        let mut app = test_app_with_worker(
            vec![
                voice(ProviderId::Azure, "ja-JP-NanamiNeural", "Nanami", "ja-JP"),
                voice(ProviderId::Google, "en-US-Studio-O", "Studio O", "en-US"),
            ],
            harness.handle,
        );

        app.request_preview();
        let first = harness.req_rx.recv().unwrap().unwrap();
        assert_eq!(first.id, 1);
        assert_eq!(first.entry.voice_id, "ja-JP-NanamiNeural");
        assert!(app.preview_busy);
        assert_eq!(app.current_id, 1);

        app.move_down();
        app.request_preview();
        assert_eq!(app.queued, Some(1));
        assert_eq!(app.status_line, "Queued next preview...");
        assert!(harness.req_rx.try_recv().is_err());

        app.handle_preview_result(PreviewResult::Err {
            id: 999,
            message: "stale".into(),
        });
        let second = harness.req_rx.recv().unwrap().unwrap();
        assert_eq!(second.id, 2);
        assert_eq!(second.entry.voice_id, "en-US-Studio-O");
        assert!(app.preview_busy);
        assert_eq!(app.current_id, 2);
        assert_eq!(app.queued, None);

        app.handle_preview_result(PreviewResult::Err {
            id: 2,
            message: "provider unavailable".into(),
        });
        assert!(!app.preview_busy);
        assert_eq!(app.current_id, 2);
        assert_eq!(app.status_line, "provider unavailable");
        assert!(app.queued.is_none());
    }

    #[test]
    fn player_cleanup_reaps_and_stops_children() {
        let mut app = test_app(vec![voice(
            ProviderId::Azure,
            "ja-JP-NanamiNeural",
            "Nanami",
            "ja-JP",
        )]);

        let mut finished = spawn_shell("exit 0");
        wait_for_exit(&mut finished);
        app.active_player = Some(finished);
        app.reap_player();
        assert!(app.active_player.is_none());

        app.active_player = Some(spawn_shell("sleep 30"));
        app.stop_player();
        assert!(app.active_player.is_none());
    }

    #[test]
    fn draw_does_not_mutate_app_state() {
        let mut app = test_app(vec![
            rich_voice(
                ProviderId::Azure,
                "ja-JP-NanamiNeural",
                "Nanami",
                &["ja-JP"],
                false,
                Some("Female"),
                Some("neural"),
                &["friendly", "neural"],
            ),
            voice(ProviderId::Google, "en-US-Studio-O", "Studio O", "en-US"),
        ]);
        app.filters.provider = Some(ProviderId::Azure);
        app.refilter();
        app.overlay = Some(FilterOverlay::new(FilterFacet::Tag));
        app.select_overlay(Some(99));
        app.toast = Some(Toast {
            message: "Copied yaml for ja-JP-NanamiNeural".into(),
            tick: 3,
        });
        app.tick = 5;
        app.preview_busy = true;
        app.queued = Some(0);
        let before = app_snapshot(&app);

        let mut view = ViewState::default();
        view.sync_from(&app);
        assert_eq!(view.list_state.selected(), app.selected);
        assert_eq!(view.overlay_list_state.selected(), Some(99));

        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &app, &mut view)).unwrap();

        assert_eq!(app_snapshot(&app), before);
    }
}

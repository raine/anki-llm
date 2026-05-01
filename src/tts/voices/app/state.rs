use std::process::Child;
use std::sync::Arc;

use ratatui::widgets::ListState;

use crate::tts::cache::TtsCache;
use crate::tui::line_input::LineInput;

use crate::tts::voices::catalog::{
    FacetCatalog, ProviderId, VoiceEntry, VoiceFilters, build_facets, filter, load_snapshot,
};
use crate::tts::voices::credentials::{ProviderPreviewState, probe_all};
use crate::tts::voices::player;
use crate::tts::voices::preview::{PreviewHandle, RequestId, spawn_worker};

pub struct InitialFilters {
    pub lang: Option<String>,
    pub provider: Option<ProviderId>,
    pub query: Option<String>,
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
    pub list_state: ListState,
}

impl FilterOverlay {
    pub(super) fn new(facet: FilterFacet) -> Self {
        let mut list_state = ListState::default();
        list_state.select(Some(0));
        Self {
            facet,
            search: LineInput::default(),
            list_state,
        }
    }

    pub(super) fn clamp_selection(&mut self, len: usize) {
        let next = if len == 0 {
            None
        } else {
            Some(self.list_state.selected().unwrap_or(0).min(len - 1))
        };
        self.list_state.select(next);
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
    pub list_state: ListState,
    pub overlay: Option<FilterOverlay>,
    pub show_help: bool,
    pub provider_states: std::collections::HashMap<ProviderId, ProviderPreviewState>,
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
    pub(super) fn new(initial: InitialFilters, cache: Arc<TtsCache>) -> Self {
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
        self.list_state
            .selected()
            .and_then(|i| self.filtered.get(i).copied())
    }

    pub(super) fn selected_entry(&self) -> Option<&VoiceEntry> {
        self.selected_index().map(|i| &self.entries[i])
    }

    pub(super) fn move_up(&mut self) {
        if self.filtered.is_empty() {
            return;
        }
        let cur = self.list_state.selected().unwrap_or(0);
        self.list_state.select(Some(cur.saturating_sub(1)));
    }

    pub(super) fn move_down(&mut self) {
        if self.filtered.is_empty() {
            return;
        }
        let cur = self.list_state.selected().unwrap_or(0);
        let next = (cur + 1).min(self.filtered.len().saturating_sub(1));
        self.list_state.select(Some(next));
    }

    pub(super) fn page_up(&mut self, rows: usize) {
        if self.filtered.is_empty() {
            return;
        }
        let cur = self.list_state.selected().unwrap_or(0);
        self.list_state.select(Some(cur.saturating_sub(rows)));
    }

    pub(super) fn page_down(&mut self, rows: usize) {
        if self.filtered.is_empty() {
            return;
        }
        let cur = self.list_state.selected().unwrap_or(0);
        let max = self.filtered.len().saturating_sub(1);
        self.list_state.select(Some((cur + rows).min(max)));
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

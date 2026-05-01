use std::sync::Arc;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

use crate::tts::voices::credentials::ProviderPreviewState;
use crate::tts::voices::player;
use crate::tts::voices::preview::{PreviewRequest, PreviewResult};
use crate::tts::voices::yaml::emit_scaffold;

use super::state::{App, FilterFacet, Toast};

impl App {
    pub(super) fn request_preview(&mut self) {
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

    pub(super) fn handle_preview_result(&mut self, result: PreviewResult) {
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
            Ok(path) => {
                self.stop_player();
                match player::spawn(&path) {
                    Ok(child) => {
                        self.active_player = Some(child);
                        self.status_line = "Playing sample...".into();
                    }
                    Err(msg) => self.status_line = format!("Player: {msg}"),
                }
            }
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
        let message = if let Ok(mut cb) = arboard::Clipboard::new()
            && cb.set_text(yaml).is_ok()
        {
            format!("Copied yaml for {}", entry.voice_id)
        } else {
            "Clipboard unavailable".to_string()
        };
        self.toast = Some(Toast {
            message,
            tick: self.tick,
        });
    }

    fn open_overlay(&mut self, facet: FilterFacet) {
        self.stop_player();
        self.overlay = Some(super::state::FilterOverlay::new(facet));
        let len = self.overlay_rows_for(facet, "").len();
        if let Some(overlay) = self.overlay.as_mut() {
            overlay.clamp_selection(len);
        }
        self.status_line = format!("{} filter", facet.title());
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

    pub(super) fn handle_paste(&mut self, text: String) {
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

    pub(super) fn handle_key(&mut self, key: KeyEvent) {
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

use std::collections::BTreeSet;

use indexmap::IndexMap;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

use crate::generate::cards::ValidatedCard;
use crate::generate::line_input::LineInput;

use super::super::theme::{Glyphs, SPINNER_FRAMES, THEME};

pub(in crate::generate::tui) struct SelectionState {
    pub(in crate::generate::tui) cards: Vec<ValidatedCard>,
    pub(in crate::generate::tui) cursor: usize,
    pub(in crate::generate::tui) selected: BTreeSet<usize>,
    pub(in crate::generate::tui) list_state: ListState,
    pub(in crate::generate::tui) detail_scroll: u16,
    /// True while a refresh (load more) request is in flight.
    pub(in crate::generate::tui) refresh_in_flight: bool,
    /// When Some, an inline term input is active for generating cards with a different term.
    pub(in crate::generate::tui) term_input: Option<LineInput>,
    /// When Some, an inline feedback input is active for regenerating the focused card.
    pub(in crate::generate::tui) feedback_input: Option<LineInput>,
    /// Card index currently being regenerated (in flight).
    pub(in crate::generate::tui) regen_in_flight: Option<usize>,
}

impl SelectionState {
    pub(in crate::generate::tui) fn new(cards: Vec<ValidatedCard>) -> Self {
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
            term_input: None,
            feedback_input: None,
            regen_in_flight: None,
        }
    }

    pub(in crate::generate::tui) fn move_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.list_state.select(Some(self.cursor));
            self.detail_scroll = 0;
        }
    }

    pub(in crate::generate::tui) fn move_down(&mut self) {
        if self.cursor + 1 < self.cards.len() {
            self.cursor += 1;
            self.list_state.select(Some(self.cursor));
            self.detail_scroll = 0;
        }
    }

    pub(in crate::generate::tui) fn toggle_current(&mut self) {
        if self
            .cards
            .get(self.cursor)
            .map(|c| c.is_duplicate)
            .unwrap_or(false)
        {
            return; // Duplicates cannot be selected (use force_toggle_duplicate)
        }
        if self.selected.contains(&self.cursor) {
            self.selected.remove(&self.cursor);
        } else {
            self.selected.insert(self.cursor);
        }
    }

    /// Force-toggle a duplicate card: clears is_duplicate so it can be selected.
    pub(in crate::generate::tui) fn force_toggle_duplicate(&mut self) {
        let Some(card) = self.cards.get_mut(self.cursor) else {
            return;
        };
        if !card.is_duplicate {
            return; // Only applies to duplicates
        }
        // Clear duplicate status and select it
        card.is_duplicate = false;
        self.selected.insert(self.cursor);
    }

    pub(in crate::generate::tui) fn select_all(&mut self) {
        for (i, c) in self.cards.iter().enumerate() {
            if !c.is_duplicate {
                self.selected.insert(i);
            }
        }
    }

    pub(in crate::generate::tui) fn select_none(&mut self) {
        self.selected.clear();
    }

    /// Remove the card at the current cursor position from the list.
    /// Returns `true` if a card was removed, `false` if the list is empty.
    pub(in crate::generate::tui) fn remove_current(&mut self) -> bool {
        if self.cards.is_empty() {
            return false;
        }

        let removed = self.cursor;
        self.cards.remove(removed);

        // Rebuild selected set: drop the removed index, shift higher indices down
        let mut new_selected = BTreeSet::new();
        for &i in &self.selected {
            if i < removed {
                new_selected.insert(i);
            } else if i > removed {
                new_selected.insert(i - 1);
            }
            // i == removed is dropped
        }
        self.selected = new_selected;

        // Adjust cursor
        if self.cards.is_empty() {
            self.cursor = 0;
        } else if self.cursor >= self.cards.len() {
            self.cursor = self.cards.len() - 1;
        }
        self.list_state.select(if self.cards.is_empty() {
            None
        } else {
            Some(self.cursor)
        });
        self.detail_scroll = 0;
        true
    }
}

pub(in crate::generate::tui) fn draw_selecting(
    frame: &mut Frame,
    state: &SelectionState,
    glyphs: &Glyphs,
    tick: u64,
    area: ratatui::layout::Rect,
) {
    // Check if cards come from multiple models (to decide whether to show model labels)
    let has_multiple_models = {
        let mut models = state.cards.iter().map(|c| c.model.as_str());
        if let Some(first) = models.next() {
            models.any(|m| m != first)
        } else {
            false
        }
    };

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
                .map(|v| crate::generate::selector::strip_html_tags(v))
                .unwrap_or_default();
            let is_regenerating = state.regen_in_flight == Some(i);
            let dup_note = if card.is_duplicate { " [dup]" } else { "" };
            let regen_note = if is_regenerating {
                let spinner = SPINNER_FRAMES[tick as usize % SPINNER_FRAMES.len()];
                format!(" {spinner}")
            } else {
                String::new()
            };
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
                Span::styled(format!("{label}{dup_note}{regen_note}"), style),
            ];
            if !flag_note.is_empty() {
                spans.push(Span::styled(
                    flag_note,
                    Style::default()
                        .fg(THEME.warning)
                        .add_modifier(Modifier::DIM),
                ));
            }
            if has_multiple_models && !card.model.is_empty() {
                spans.push(Span::styled(
                    format!(" [{}]", card.model),
                    Style::default()
                        .fg(THEME.dimmed)
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
                "  ⚠ Already exists in Anki (f to force-select)",
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

        if card.is_duplicate {
            if let Some(ref dup_fields) = card.duplicate_fields {
                // Diff view: show field-by-field comparison
                render_diff_lines(&mut lines, &card.raw_anki_fields, dup_fields);
            } else {
                // No duplicate fields available, show new card normally
                for (name, value) in &card.raw_anki_fields {
                    lines.push(Line::from(Span::styled(
                        name.clone(),
                        Style::default().fg(THEME.info).add_modifier(Modifier::BOLD),
                    )));
                    lines.extend(crate::generate::selector::markdown_to_lines(value, "  "));
                    lines.push(Line::from(""));
                }
            }
        } else {
            for (name, value) in &card.raw_anki_fields {
                lines.push(Line::from(Span::styled(
                    name.clone(),
                    Style::default().fg(THEME.info).add_modifier(Modifier::BOLD),
                )));
                lines.extend(crate::generate::selector::markdown_to_lines(value, "  "));
                lines.push(Line::from(""));
            }
        }

        if has_multiple_models && !card.model.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("Model: {}", card.model),
                Style::default().fg(THEME.dimmed),
            )));
        }

        let detail_para = Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: false })
            .scroll((state.detail_scroll, 0));
        frame.render_widget(detail_para, chunks[1]);
    }

    // Term input popup overlay
    if let Some(ref input) = state.term_input {
        draw_term_input_overlay(frame, input, area);
    }

    // Feedback input overlay for card regeneration
    if let Some(ref input) = state.feedback_input {
        draw_feedback_overlay(frame, input, area);
    }
}

fn draw_feedback_overlay(frame: &mut Frame, input: &LineInput, area: ratatui::layout::Rect) {
    let max_width = 60u16.min(area.width.saturating_sub(4));
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

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Regenerate feedback ")
        .border_style(Style::default().fg(THEME.info));

    let para = Paragraph::new(input.value())
        .block(block)
        .scroll((0, scroll as u16));
    frame.render_widget(Clear, input_area);
    frame.render_widget(para, input_area);

    frame.set_cursor_position((
        input_area.x + 1 + (input.visual_cursor().saturating_sub(scroll)) as u16,
        input_area.y + 1,
    ));
}

fn draw_term_input_overlay(frame: &mut Frame, input: &LineInput, area: ratatui::layout::Rect) {
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

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Enter term ")
        .border_style(Style::default().fg(THEME.info));

    let para = Paragraph::new(input.value())
        .block(block)
        .scroll((0, scroll as u16));
    frame.render_widget(Clear, input_area);
    frame.render_widget(para, input_area);

    frame.set_cursor_position((
        input_area.x + 1 + (input.visual_cursor().saturating_sub(scroll)) as u16,
        input_area.y + 1,
    ));
}

/// Render a field-by-field diff between the new card and the existing Anki note.
fn render_diff_lines<'a>(
    lines: &mut Vec<Line<'a>>,
    new_fields: &IndexMap<String, String>,
    existing_fields: &IndexMap<String, String>,
) {
    let existing_style = Style::default().fg(THEME.danger);
    let new_style = Style::default().fg(THEME.success);

    for (name, new_value) in new_fields {
        let existing_value = existing_fields.get(name).map(|s| s.as_str()).unwrap_or("");
        let new_plain = crate::generate::selector::strip_html_tags(new_value);
        let existing_plain = crate::generate::selector::strip_html_tags(existing_value);

        lines.push(Line::from(Span::styled(
            name.clone(),
            Style::default().fg(THEME.info).add_modifier(Modifier::BOLD),
        )));

        if new_plain.trim() == existing_plain.trim() {
            // Fields match — show normally
            lines.extend(crate::generate::selector::markdown_to_lines(
                new_value, "  ",
            ));
        } else {
            // Fields differ — show existing (red) then new (green)
            for line_str in existing_plain.lines() {
                lines.push(Line::from(Span::styled(
                    format!("  - {line_str}"),
                    existing_style,
                )));
            }
            for line_str in new_plain.lines() {
                lines.push(Line::from(Span::styled(
                    format!("  + {line_str}"),
                    new_style,
                )));
            }
        }
        lines.push(Line::from(""));
    }

    // Show fields that exist in Anki but not in the new card
    for (name, value) in existing_fields {
        if !new_fields.contains_key(name) {
            let plain = crate::generate::selector::strip_html_tags(value);
            if plain.trim().is_empty() {
                continue;
            }
            lines.push(Line::from(Span::styled(
                name.clone(),
                Style::default()
                    .fg(THEME.info)
                    .add_modifier(Modifier::BOLD | Modifier::DIM),
            )));
            for line_str in plain.lines() {
                lines.push(Line::from(Span::styled(
                    format!("  - {line_str}"),
                    existing_style,
                )));
            }
            lines.push(Line::from(""));
        }
    }
}

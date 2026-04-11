use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

use crate::llm::pricing;

use super::theme::THEME;

pub struct ModelPickerState {
    pub models: Vec<String>,
    pub filter: String,
    pub cursor: usize,
    pub list_state: ListState,
}

impl ModelPickerState {
    pub fn new(models: Vec<String>, current_model: Option<&str>) -> Self {
        let cursor = current_model
            .and_then(|m| models.iter().position(|s| s == m))
            .unwrap_or(0);
        let mut list_state = ListState::default();
        list_state.select(Some(cursor));
        Self {
            models,
            filter: String::new(),
            cursor,
            list_state,
        }
    }

    pub fn filtered_models(&self) -> Vec<&str> {
        if self.filter.is_empty() {
            self.models.iter().map(|s| s.as_str()).collect()
        } else {
            let normalized_filter: String = self
                .filter
                .to_lowercase()
                .chars()
                .filter(|c| *c != '-' && *c != '.')
                .collect();
            self.models
                .iter()
                .filter(|m| {
                    let normalized: String = m
                        .to_lowercase()
                        .chars()
                        .filter(|c| *c != '-' && *c != '.')
                        .collect();
                    normalized.contains(&normalized_filter)
                })
                .map(|s| s.as_str())
                .collect()
        }
    }

    pub fn move_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.list_state.select(Some(self.cursor));
        }
    }

    pub fn move_down(&mut self) {
        let filtered = self.filtered_models();
        if self.cursor + 1 < filtered.len() {
            self.cursor += 1;
            self.list_state.select(Some(self.cursor));
        }
    }

    pub fn selected(&self) -> Option<&str> {
        let filtered = self.filtered_models();
        filtered.get(self.cursor).copied()
    }

    pub fn add_filter_char(&mut self, c: char) {
        self.filter.push(c);
        self.clamp_cursor();
    }

    pub fn remove_filter_char(&mut self) {
        self.filter.pop();
        self.clamp_cursor();
    }

    fn clamp_cursor(&mut self) {
        let len = self.filtered_models().len();
        if len == 0 {
            self.cursor = 0;
        } else if self.cursor >= len {
            self.cursor = len - 1;
        }
        self.list_state.select(Some(self.cursor));
    }
}

pub fn draw_model_picker(frame: &mut Frame, picker: &ModelPickerState) {
    let filtered = picker.filtered_models();
    let row_count = filtered.len() as u16;
    let height = (row_count + 2).min(20); // borders
    let width: u16 = 48;

    let area = frame.area();
    let rect = Rect::new(
        area.width.saturating_sub(width) / 2,
        area.height.saturating_sub(height) / 2,
        width.min(area.width),
        height.min(area.height),
    );

    let title = if picker.filter.is_empty() {
        Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled(
                "Model",
                Style::default()
                    .fg(THEME.header)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ", Style::default()),
        ])
    } else {
        Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled(
                "Model",
                Style::default()
                    .fg(THEME.header)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" [", Style::default().fg(THEME.dimmed)),
            Span::styled(
                picker.filter.as_str(),
                Style::default().fg(THEME.highlight_fg),
            ),
            Span::styled("] ", Style::default().fg(THEME.dimmed)),
        ])
    };

    let block = Block::bordered()
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(THEME.help_border))
        .title(title)
        .title_bottom(Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled("Enter", Style::default().fg(THEME.dimmed)),
            Span::styled(" select ", Style::default().fg(THEME.help_muted)),
            Span::styled("Esc", Style::default().fg(THEME.dimmed)),
            Span::styled(" cancel ", Style::default().fg(THEME.help_muted)),
        ]));

    // Inner width: total width - 2 borders - 2 highlight symbol
    let inner_w = width.saturating_sub(4) as usize;

    let items: Vec<ListItem> = filtered
        .iter()
        .map(|m| {
            let price = pricing::model_pricing(m)
                .map(|p| format_model_price(p.input_cost_per_million, p.output_cost_per_million))
                .unwrap_or_default();
            let pad = inner_w.saturating_sub(m.len() + price.len());
            ListItem::new(Line::from(vec![
                Span::styled(*m, Style::default().fg(THEME.text)),
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

pub fn draw_log_panel(frame: &mut Frame, logs: &[String], log_scroll: u16, area: Rect) {
    let visible_height = area.height.saturating_sub(2) as usize;
    let total_logs = logs.len();
    let scroll_pos = log_scroll as usize;
    // Show a window of logs ending at scroll_pos (inclusive)
    let end = (scroll_pos + 1).min(total_logs);
    let start = end.saturating_sub(visible_height);

    let log_text: Text = logs[start..end]
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

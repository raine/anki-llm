use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::generate::process::FlaggedCard;

use crate::tui::theme::THEME;

pub(in crate::generate::tui) struct ReviewState {
    pub(in crate::generate::tui) flagged: Vec<FlaggedCard>,
    pub(in crate::generate::tui) cursor: usize,
    pub(in crate::generate::tui) decisions: Vec<bool>,
    pub(in crate::generate::tui) detail_scroll: u16,
}

impl ReviewState {
    pub(in crate::generate::tui) fn new(flagged: Vec<FlaggedCard>) -> Self {
        let len = flagged.len();
        Self {
            flagged,
            cursor: 0,
            decisions: vec![false; len],
            detail_scroll: 0,
        }
    }

    pub(in crate::generate::tui) fn current(&self) -> Option<&FlaggedCard> {
        self.flagged.get(self.cursor)
    }

    pub(in crate::generate::tui) fn keep_current(&mut self) {
        if self.cursor < self.decisions.len() {
            self.decisions[self.cursor] = true;
        }
        self.advance();
    }

    pub(in crate::generate::tui) fn discard_current(&mut self) {
        if self.cursor < self.decisions.len() {
            self.decisions[self.cursor] = false;
        }
        self.advance();
    }

    pub(in crate::generate::tui) fn keep_all(&mut self) {
        for d in &mut self.decisions {
            *d = true;
        }
        self.cursor = self.flagged.len(); // done
    }

    pub(in crate::generate::tui) fn discard_all(&mut self) {
        for d in &mut self.decisions {
            *d = false;
        }
        self.cursor = self.flagged.len(); // done
    }

    pub(in crate::generate::tui) fn advance(&mut self) {
        self.cursor += 1;
        self.detail_scroll = 0;
    }

    pub(in crate::generate::tui) fn move_back(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.detail_scroll = 0;
        }
    }

    pub(in crate::generate::tui) fn is_done(&self) -> bool {
        self.cursor >= self.flagged.len()
    }
}

pub(in crate::generate::tui) fn draw_reviewing(
    frame: &mut Frame,
    state: &ReviewState,
    area: ratatui::layout::Rect,
) {
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
            lines.extend(crate::generate::selector::markdown_to_lines(value, "  "));
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

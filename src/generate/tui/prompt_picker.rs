use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};

use super::theme::{Glyphs, THEME};

pub(super) fn run_prompt_picker(
    mut terminal: DefaultTerminal,
    prompts: &[crate::workspace::discovery::PromptEntry],
    glyphs: &Glyphs,
) -> Option<PathBuf> {
    use crate::workspace::resolver::last_prompt;

    let mut cursor: usize = 0;
    let mut list_state = ListState::default();

    // Pre-select last-used prompt if it matches one of the entries
    if let Some(last) = last_prompt()
        && let Some(idx) = prompts.iter().position(|p| p.path == last)
    {
        cursor = idx;
    }
    list_state.select(Some(cursor));

    loop {
        terminal
            .draw(|f| draw_prompt_picker(f, prompts, cursor, &mut list_state, glyphs))
            .ok();

        if event::poll(Duration::from_millis(50)).unwrap_or(false)
            && let Ok(Event::Key(key)) = event::read()
        {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    if cursor > 0 {
                        cursor -= 1;
                        list_state.select(Some(cursor));
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if cursor + 1 < prompts.len() {
                        cursor += 1;
                        list_state.select(Some(cursor));
                    }
                }
                KeyCode::Enter => {
                    return Some(prompts[cursor].path.clone());
                }
                KeyCode::Esc | KeyCode::Char('q') => {
                    return None;
                }
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return None;
                }
                _ => {}
            }
        }
    }
}

fn draw_prompt_picker(
    frame: &mut Frame,
    prompts: &[crate::workspace::discovery::PromptEntry],
    cursor: usize,
    list_state: &mut ListState,
    _glyphs: &Glyphs,
) {
    let area = frame.area();

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(area);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[0]);

    // Left: prompt list
    let items: Vec<ListItem> = prompts
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let style = if i == cursor {
                Style::default().fg(THEME.info).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(THEME.text)
            };
            let deck_info = &p.deck;
            ListItem::new(Line::from(vec![
                Span::styled(&p.title, style),
                Span::styled(format!("  {deck_info}"), Style::default().fg(THEME.dimmed)),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(THEME.border))
                .title(Span::styled(
                    " Select Prompt ",
                    Style::default().fg(THEME.info).add_modifier(Modifier::BOLD),
                )),
        )
        .highlight_style(
            Style::default()
                .bg(THEME.highlight_bg)
                .fg(THEME.info)
                .add_modifier(Modifier::BOLD),
        );

    frame.render_stateful_widget(list, cols[0], list_state);

    // Right: detail panel for selected prompt
    let selected = &prompts[cursor];
    let mut detail_lines = vec![
        Line::from(vec![
            Span::styled("Title: ", Style::default().fg(THEME.dimmed)),
            Span::styled(&selected.title, Style::default().fg(THEME.text)),
        ]),
        Line::from(vec![
            Span::styled("Deck: ", Style::default().fg(THEME.dimmed)),
            Span::styled(selected.deck.as_str(), Style::default().fg(THEME.text)),
        ]),
        Line::from(vec![
            Span::styled("Note type: ", Style::default().fg(THEME.dimmed)),
            Span::styled(selected.note_type.as_str(), Style::default().fg(THEME.text)),
        ]),
    ];
    if let Some(ref desc) = selected.description {
        detail_lines.push(Line::from(""));
        detail_lines.push(Line::from(Span::styled(
            desc.as_str(),
            Style::default().fg(THEME.text),
        )));
    }
    detail_lines.push(Line::from(""));
    detail_lines.push(Line::from(Span::styled(
        selected.path.display().to_string(),
        Style::default().fg(THEME.dimmed),
    )));

    let detail = Paragraph::new(detail_lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(THEME.border))
                .title(Span::styled(" Details ", Style::default().fg(THEME.header))),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(detail, cols[1]);

    // Footer
    let footer = Line::from(vec![
        Span::styled("↑↓", Style::default().fg(THEME.dimmed)),
        Span::styled(
            " Navigate",
            Style::default().fg(THEME.text).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" │ ", Style::default().fg(THEME.border)),
        Span::styled("Enter", Style::default().fg(THEME.dimmed)),
        Span::styled(
            " Select",
            Style::default().fg(THEME.text).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" │ ", Style::default().fg(THEME.border)),
        Span::styled("Esc", Style::default().fg(THEME.dimmed)),
        Span::styled(
            " Quit",
            Style::default().fg(THEME.text).add_modifier(Modifier::BOLD),
        ),
    ]);
    frame.render_widget(Paragraph::new(footer), rows[1]);
}

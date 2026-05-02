use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Clear, Row, Table};

use super::super::runner::done_audio_cache_path;
use super::super::state::{App, AppMode};

use crate::tui::theme::THEME;

pub(in crate::generate::tui) fn draw_help_overlay(frame: &mut Frame, app: &App) {
    let shortcuts: Vec<(&str, &str)> = match &app.mode {
        AppMode::Input(_) => vec![
            ("Enter", "Generate"),
            ("Tab", "Queue term"),
            ("Ctrl+O", "Model"),
            ("↑ / ↓", "History"),
            ("Ctrl+P", "Switch prompt"),
            ("Esc", "Clear"),
            ("Ctrl+C", "Quit"),
        ],
        AppMode::Running => vec![("Esc", "Cancel"), ("q", "Quit")],
        AppMode::Selecting(_) => {
            let mut v = vec![
                ("Space", "Toggle"),
                ("f", "Force-select duplicate"),
                ("a", "All"),
                ("n", "None"),
                ("c", "Copy"),
                ("d", "Remove"),
                ("e", "Edit in $EDITOR"),
            ];
            if app
                .session_info
                .as_ref()
                .map(|info| info.post_select_configured)
                .unwrap_or(false)
            {
                v.push(("z", "Skip post-select"));
            }
            v.extend(vec![
                ("r", "More"),
                ("t", "More (new term)"),
                ("R", "Regenerate card"),
            ]);
            if app
                .session_info
                .as_ref()
                .map(|info| info.tts_configured)
                .unwrap_or(false)
                && app.audio_ready()
            {
                v.push(("p", "Play audio"));
            }
            v.extend([
                ("Ctrl+O", "Model"),
                ("Enter", "Confirm"),
                ("Esc", "Back"),
                ("q / Ctrl+C", "Quit"),
                ("PgUp/PgDn", "Scroll"),
            ]);
            v
        }
        AppMode::Reviewing(_) => vec![
            ("k / y", "Keep"),
            ("d / n", "Discard"),
            ("a", "Keep all"),
            ("x", "Discard all"),
            ("u", "Back"),
            ("q", "Quit"),
        ],
        AppMode::Done {
            note_ids, cards, ..
        } => {
            let mut v = vec![
                ("j / k", "Browse steps"),
                ("PgUp/PgDn", "Scroll logs"),
                ("Esc", "Back to summary"),
                ("n", "New term"),
                ("r", "Retry"),
                ("Ctrl+O", "Model"),
            ];
            if app
                .session_info
                .as_ref()
                .map(|info| info.tts_configured)
                .unwrap_or(false)
                && app.audio_ready()
                && cards.first().and_then(done_audio_cache_path).is_some()
            {
                v.push(("p", "Play audio"));
            }
            if !note_ids.is_empty() {
                v.push(("d", "Delete from Anki"));
            }
            v.push(("q", "Quit"));
            v
        }
        AppMode::Error(_) => {
            vec![
                ("j / k", "Browse steps"),
                ("PgUp/PgDn", "Scroll logs"),
                ("Esc", "Back to summary"),
                ("n", "New term"),
                ("r", "Retry"),
                ("Ctrl+O", "Model"),
                ("q", "Quit"),
            ]
        }
    };

    let row_count = shortcuts.len() as u16;
    let height = row_count + 5; // borders + padding + empty line at top
    let width: u16 = 44;

    let area = frame.area();
    let rect = Rect::new(
        area.width.saturating_sub(width) / 2,
        area.height.saturating_sub(height) / 2,
        width.min(area.width),
        height.min(area.height),
    );

    let mode_title = match &app.mode {
        AppMode::Input(_) => "Input",
        AppMode::Running => "Running",
        AppMode::Selecting(_) => "Select",
        AppMode::Reviewing(_) => "Review",
        AppMode::Done { .. } => "Done",
        AppMode::Error(_) => "Error",
    };

    let block = Block::bordered()
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(THEME.help_border))
        .title(Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled(
                mode_title,
                Style::default()
                    .fg(THEME.header)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ", Style::default()),
        ]))
        .title_bottom(Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled("any key", Style::default().fg(THEME.dimmed)),
            Span::styled(" to close ", Style::default().fg(THEME.help_muted)),
        ]));

    let mut rows: Vec<Row> = vec![Row::new(vec![Cell::from(""), Cell::from("")])];
    rows.extend(shortcuts.into_iter().map(|(key, desc)| {
        Row::new(vec![
            Cell::from(Line::from(vec![
                Span::styled(" ", Style::default()),
                Span::styled(
                    format!("{:>8}", key),
                    Style::default()
                        .fg(THEME.dimmed)
                        .add_modifier(Modifier::BOLD),
                ),
            ])),
            Cell::from(Line::from(vec![
                Span::styled(" · ", Style::default().fg(THEME.help_muted)),
                Span::styled(desc, Style::default().fg(THEME.text)),
            ])),
        ])
    }));

    let table = Table::new(rows, [Constraint::Length(10), Constraint::Min(25)])
        .block(block)
        .column_spacing(0);

    frame.render_widget(Clear, rect);
    frame.render_widget(table, rect);
}

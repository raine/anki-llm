use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table};

use super::events::StepStatus;
use super::runner::done_audio_cache_path;
use super::state::{App, AppMode};

use crate::generate::pipeline::PipelineStep;
use crate::llm::pricing;
use crate::tui::theme::{SPINNER_FRAMES, THEME, footer_cmd, footer_pipe};

pub(super) fn draw_help_overlay(frame: &mut Frame, app: &App) {
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
                && app.player.is_some()
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
                && app.player.is_some()
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

pub(super) fn draw_sidebar(frame: &mut Frame, app: &App, area: Rect) {
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(THEME.border));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let has_term = app.last_term.is_some() && !matches!(app.mode, AppMode::Input(_));
    let info_height: u16 = match (app.session_info.is_some(), has_term) {
        (true, true) => 5,
        (true, false) => 4,
        _ => 0,
    };

    // Build step lines (detail on second indented line)
    let spinner_frame = format!(
        "{} ",
        SPINNER_FRAMES[app.tick as usize % SPINNER_FRAMES.len()]
    );

    let is_browsing = matches!(app.mode, AppMode::Done { .. } | AppMode::Error(_));

    let mut step_lines: Vec<Line> = Vec::new();
    for (i, record) in app.steps.iter().enumerate() {
        let step = &record.step;
        let status = &record.status;
        let is_interactive = matches!(step, PipelineStep::Select | PipelineStep::QualityCheck);
        let (icon, mut style): (&str, Style) = match status {
            StepStatus::Pending => ("  ", Style::default().fg(THEME.dimmed)),
            StepStatus::Running(_) if is_interactive => ("▸ ", Style::default().fg(THEME.info)),
            StepStatus::Running(_) => (&spinner_frame, Style::default().fg(THEME.info)),
            StepStatus::Done(_) => ("✓ ", Style::default().fg(THEME.success)),
            StepStatus::Skipped => ("- ", Style::default().fg(THEME.dimmed)),
            StepStatus::Error(_) => ("✗ ", Style::default().fg(THEME.danger)),
        };

        // Highlight selected step in browse mode
        let is_selected = is_browsing && app.browse_step == Some(i);
        if is_selected {
            style = style.bg(THEME.highlight_bg);
        }

        let detail = match status {
            StepStatus::Running(Some(d)) | StepStatus::Done(Some(d)) => Some(d.as_str()),
            StepStatus::Error(_) => None,
            _ => None,
        };

        // Show detail inline if it fits, otherwise on a second line
        let sidebar_inner = 28; // 30 - border - padding
        if let Some(d) = detail {
            let inline_len = icon.len() + step.label().len() + 2 + d.len();
            if inline_len <= sidebar_inner {
                step_lines.push(Line::from(vec![
                    Span::styled(icon, style),
                    Span::styled(step.label(), style),
                    Span::styled(
                        format!("  {d}"),
                        Style::default().fg(THEME.dimmed).bg(if is_selected {
                            THEME.highlight_bg
                        } else {
                            Color::Reset
                        }),
                    ),
                ]));
            } else {
                step_lines.push(Line::from(vec![
                    Span::styled(icon, style),
                    Span::styled(step.label(), style),
                ]));
                step_lines.push(Line::from(Span::styled(
                    format!("    {d}"),
                    Style::default().fg(THEME.dimmed),
                )));
            }
        } else {
            step_lines.push(Line::from(vec![
                Span::styled(icon, style),
                Span::styled(step.label(), style),
            ]));
        }
    }

    let steps_height = step_lines.len() as u16;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(info_height),
            Constraint::Length(steps_height),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(inner);

    // Session info
    if let Some(info) = &app.session_info {
        let mut lines = vec![
            Line::from(vec![
                Span::styled("Deck  ", Style::default().fg(THEME.dimmed)),
                Span::raw(&info.deck),
            ]),
            Line::from(vec![
                Span::styled("Note  ", Style::default().fg(THEME.dimmed)),
                Span::raw(&info.note_type),
            ]),
            Line::from(vec![
                Span::styled("Model ", Style::default().fg(THEME.dimmed)),
                Span::raw(&info.model),
            ]),
        ];
        if let Some(term) = &app.last_term
            && has_term
        {
            let label = if let Some((current, total)) = app.batch_progress {
                format!("{current}/{total} ")
            } else {
                String::new()
            };
            lines.push(Line::from(vec![
                Span::styled("Term  ", Style::default().fg(THEME.dimmed)),
                Span::styled(label, Style::default().fg(THEME.info)),
                Span::styled(
                    term.clone(),
                    Style::default().fg(THEME.text).add_modifier(Modifier::BOLD),
                ),
            ]));
        }
        frame.render_widget(Paragraph::new(lines), chunks[0]);
    }

    // Pipeline steps
    frame.render_widget(Paragraph::new(step_lines), chunks[1]);

    // Cost
    let total = app.session_cost + app.run_cost;
    if total > 0.0 {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                pricing::format_cost(total),
                Style::default().fg(THEME.dimmed),
            ))),
            chunks[3],
        );
    }
}

pub(super) fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let mut s: Vec<Span<'static>> = vec![Span::raw(" ")];

    match &app.mode {
        AppMode::Input(_) => {
            s.extend(footer_cmd("Enter", "Generate"));
            s.push(footer_pipe());
            s.extend(footer_cmd("Tab", "Queue term"));
            s.push(footer_pipe());
            s.extend(footer_cmd("Ctrl+O", "Model"));
            s.push(footer_pipe());
            s.extend(footer_cmd("↑↓", "History"));
            s.push(footer_pipe());
            s.extend(footer_cmd("Ctrl+C", "Quit"));
        }
        AppMode::Running => {
            if let Some((current, total)) = app.batch_progress {
                let spinner = SPINNER_FRAMES[app.tick as usize % SPINNER_FRAMES.len()];
                s.push(Span::styled(
                    format!("{spinner} Batch {current}/{total}"),
                    Style::default().fg(THEME.info),
                ));
                s.push(footer_pipe());
            }
            s.extend(footer_cmd("Esc", "Cancel"));
            s.push(footer_pipe());
            s.extend(footer_cmd("q", "Quit"));
            s.push(footer_pipe());
            s.extend(footer_cmd("?", "Help"));
        }
        AppMode::Selecting(state) => {
            let n = state.selected.len();
            let focused_is_dup = state
                .cards
                .get(state.cursor)
                .map(|c| c.is_duplicate)
                .unwrap_or(false);
            s.extend(footer_cmd("Space", "Toggle"));
            s.push(footer_pipe());
            if focused_is_dup {
                s.extend(footer_cmd("f", "Force"));
                s.push(footer_pipe());
            }
            s.extend(footer_cmd("a", "All"));
            s.push(footer_pipe());
            s.extend(footer_cmd("n", "None"));
            s.push(footer_pipe());
            s.extend(footer_cmd("c", "Copy"));
            s.push(footer_pipe());
            s.extend(footer_cmd("d", "Remove"));
            s.push(footer_pipe());
            s.extend(footer_cmd("e", "Edit"));
            s.push(footer_pipe());
            let has_post_select = app
                .session_info
                .as_ref()
                .map(|info| info.post_select_configured)
                .unwrap_or(false);
            if has_post_select {
                s.extend(footer_cmd("z", "Skip post"));
                s.push(footer_pipe());
            }
            if app
                .session_info
                .as_ref()
                .map(|info| info.tts_configured)
                .unwrap_or(false)
                && app.player.is_some()
            {
                s.extend(footer_cmd("p", "Play"));
                s.push(footer_pipe());
            }
            if state.refresh_in_flight || state.regen_in_flight.is_some() {
                let spinner = SPINNER_FRAMES[app.tick as usize % SPINNER_FRAMES.len()];
                let loading_text = if let Some((current, total)) = app.batch_progress {
                    format!("{spinner} Batch {current}/{total}...")
                } else {
                    format!("{spinner} Loading...")
                };
                s.push(Span::styled(loading_text, Style::default().fg(THEME.info)));
            } else if state.term_input.is_some() || state.feedback_input.is_some() {
                s.extend(footer_cmd("Enter", "Submit"));
                s.push(footer_pipe());
                s.extend(footer_cmd("Esc", "Cancel"));
            } else {
                s.extend(footer_cmd("r", "More"));
                s.push(footer_pipe());
                s.extend(footer_cmd("t", "New term"));
                s.push(footer_pipe());
                s.extend(footer_cmd("R", "Regen"));
            }
            s.push(footer_pipe());
            s.extend(footer_cmd("Ctrl+O", "Model"));
            s.push(footer_pipe());
            s.extend(footer_cmd("Enter", "Confirm"));
            s.push(footer_pipe());
            s.extend(footer_cmd("Esc", "Back"));
            s.push(footer_pipe());
            s.extend(footer_cmd("q", "Quit"));
            s.push(footer_pipe());
            s.extend(footer_cmd("?", "Help"));
            s.push(Span::styled(
                format!("  ({n} selected)"),
                Style::default().fg(THEME.dimmed),
            ));
        }
        AppMode::Reviewing(state) => {
            let cur = (state.cursor + 1).min(state.flagged.len());
            let total = state.flagged.len();
            s.push(Span::styled(
                format!("Flagged {cur}/{total}"),
                Style::default().fg(THEME.warning),
            ));
            s.push(footer_pipe());
            s.extend(footer_cmd("k", "Keep"));
            s.push(footer_pipe());
            s.extend(footer_cmd("d", "Discard"));
            s.push(footer_pipe());
            s.extend(footer_cmd("u", "Back"));
            s.push(footer_pipe());
            s.extend(footer_cmd("a", "Keep all"));
            s.push(footer_pipe());
            s.extend(footer_cmd("x", "Discard all"));
            s.push(footer_pipe());
            s.extend(footer_cmd("q", "Quit"));
            s.push(footer_pipe());
            s.extend(footer_cmd("?", "Help"));
        }
        AppMode::Done {
            note_ids, cards, ..
        } => {
            s.extend(footer_cmd("j/k", "Steps"));
            s.push(footer_pipe());
            if !app.is_fatal {
                s.extend(footer_cmd("n", "New term"));
                if app.last_term.is_some() {
                    s.push(footer_pipe());
                    s.extend(footer_cmd("r", "Retry"));
                }
                s.push(footer_pipe());
                s.extend(footer_cmd("Ctrl+O", "Model"));
                if !cards.is_empty() {
                    s.push(footer_pipe());
                    s.extend(footer_cmd("c", "Copy"));
                }
                if app
                    .session_info
                    .as_ref()
                    .map(|info| info.tts_configured)
                    .unwrap_or(false)
                    && app.player.is_some()
                    && cards.first().and_then(done_audio_cache_path).is_some()
                {
                    s.push(footer_pipe());
                    s.extend(footer_cmd("p", "Play"));
                }
                if !note_ids.is_empty() {
                    s.push(footer_pipe());
                    s.extend(footer_cmd("d", "Delete"));
                }
                s.push(footer_pipe());
            }
            s.extend(footer_cmd("q", "Quit"));
            s.push(footer_pipe());
            s.extend(footer_cmd("?", "Help"));
        }
        AppMode::Error(_) => {
            s.extend(footer_cmd("j/k", "Steps"));
            s.push(footer_pipe());
            if !app.is_fatal {
                s.extend(footer_cmd("n", "New term"));
                if app.last_term.is_some() {
                    s.push(footer_pipe());
                    s.extend(footer_cmd("r", "Retry"));
                }
                s.push(footer_pipe());
                s.extend(footer_cmd("Ctrl+O", "Model"));
                s.push(footer_pipe());
            }
            s.extend(footer_cmd("q", "Quit"));
            s.push(footer_pipe());
            s.extend(footer_cmd("?", "Help"));
        }
    }

    frame.render_widget(Paragraph::new(Line::from(s)), area);
}

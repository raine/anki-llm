use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use super::super::events::StepStatus;
use super::super::state::{App, AppMode};

use crate::generate::pipeline::PipelineStep;
use crate::llm::pricing;
use crate::tui::theme::{SPINNER_FRAMES, THEME};

pub(in crate::generate::tui) fn draw_sidebar(frame: &mut Frame, app: &App, area: Rect) {
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

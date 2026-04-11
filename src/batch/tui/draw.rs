use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Cell, Gauge, Paragraph, Row, Table, Wrap};

use crate::llm::pricing;
use crate::tui::theme::{SPINNER_FRAMES, THEME, footer_cmd, footer_pipe};

use super::super::events::{BatchPlan, RowState};
use super::state::{AppMode, DoneState, RunState};

pub fn draw(mode: &AppMode, plan: &BatchPlan, frame: &mut Frame) {
    match mode {
        AppMode::Preflight => draw_preflight(plan, frame),
        AppMode::Running(state) => draw_running(state, plan, frame),
        AppMode::Done(state) => draw_done(state, plan, frame),
        AppMode::Error(msg) => draw_error(msg, frame),
    }
}

// ---------------------------------------------------------------------------
// Preflight
// ---------------------------------------------------------------------------

fn draw_preflight(plan: &BatchPlan, frame: &mut Frame) {
    let area = frame.area();

    // Build content lines
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(""));

    let label_style = Style::default().fg(THEME.dimmed);
    let value_style = Style::default().fg(THEME.text);

    let fields: Vec<(&str, String)> = vec![
        ("Prompt", plan.prompt_path.clone()),
        ("Model", plan.model.clone()),
        (
            "Mode",
            match &plan.output_mode {
                super::super::events::OutputMode::SingleField(f) => {
                    format!("single field ({f})")
                }
                super::super::events::OutputMode::JsonMerge => "JSON merge".to_string(),
            },
        ),
        ("Batch size", plan.batch_size.to_string()),
        ("Retries", plan.retries.to_string()),
    ];

    for (label, value) in &fields {
        let pad = 12usize.saturating_sub(label.len());
        lines.push(Line::from(vec![
            Span::styled(format!("  {label}"), label_style),
            Span::raw(" ".repeat(pad)),
            Span::styled(value.as_str(), value_style),
        ]));
    }

    lines.push(Line::from(""));

    let input_label = format!("{} ({} rows)", plan.output_path, plan.input_total);
    let file_fields: Vec<(&str, String)> =
        vec![("Input", input_label), ("Output", plan.output_path.clone())];
    for (label, value) in &file_fields {
        let pad = 12usize.saturating_sub(label.len());
        lines.push(Line::from(vec![
            Span::styled(format!("  {label}"), label_style),
            Span::raw(" ".repeat(pad)),
            Span::styled(value.as_str(), value_style),
        ]));
    }

    if plan.resume_skipped > 0 {
        lines.push(Line::from(vec![
            Span::styled("  Resuming", label_style),
            Span::raw("    "),
            Span::styled(
                format!("{} rows from prior output", plan.resume_skipped),
                Style::default().fg(THEME.info),
            ),
        ]));
    }

    lines.push(Line::from(vec![
        Span::styled("  To process", label_style),
        Span::raw("  "),
        Span::styled(
            format!("{} rows", plan.run_total),
            Style::default().fg(THEME.text).add_modifier(Modifier::BOLD),
        ),
    ]));

    // Sample prompt
    if let Some(ref sample) = plan.sample_prompt {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("  {} Sample prompt (row 1) {}", "───", "─".repeat(20)),
            Style::default().fg(THEME.border),
        )));
        for line in sample.lines().take(10) {
            lines.push(Line::from(Span::styled(
                format!("  {line}"),
                Style::default().fg(THEME.dimmed),
            )));
        }
        if sample.lines().count() > 10 {
            lines.push(Line::from(Span::styled(
                "  ...",
                Style::default().fg(THEME.dimmed),
            )));
        }
    }

    lines.push(Line::from(""));

    let text = Text::from(lines);
    let block = Block::bordered()
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(THEME.border));
    let para = Paragraph::new(text).block(block);

    // Footer
    let footer_spans: Vec<Span> = [
        footer_cmd("Enter", "Start"),
        vec![footer_pipe()],
        footer_cmd("Esc", "Cancel"),
    ]
    .concat();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);

    frame.render_widget(para, chunks[0]);
    frame.render_widget(Line::from(footer_spans), chunks[1]);
}

// ---------------------------------------------------------------------------
// Running
// ---------------------------------------------------------------------------

fn draw_running(state: &RunState, plan: &BatchPlan, frame: &mut Frame) {
    let area = frame.area();

    // Main layout: footer at bottom
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(area);

    let body = main_chunks[0];
    let log_area = main_chunks[1];
    let footer_area = main_chunks[2];

    // Body: sidebar + table
    let body_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(24), Constraint::Min(1)])
        .split(body);

    let sidebar_area = body_chunks[0];
    let table_area = body_chunks[1];

    // --- Sidebar ---
    draw_sidebar(state, plan, frame, sidebar_area);

    // --- Row table ---
    draw_row_table(state, frame, table_area);

    // --- Log strip ---
    draw_log_strip(state, frame, log_area);

    // --- Footer ---
    let mut footer_spans: Vec<Span> = Vec::new();
    if state.cancelling {
        footer_spans.push(Span::styled(
            "Cancelling...",
            Style::default().fg(THEME.warning),
        ));
    } else {
        footer_spans.extend(footer_cmd("Esc", "Cancel"));
        footer_spans.push(footer_pipe());
        footer_spans.extend(footer_cmd("j/k", "Scroll"));
    }
    frame.render_widget(Line::from(footer_spans), footer_area);
}

fn draw_sidebar(state: &RunState, plan: &BatchPlan, frame: &mut Frame, area: Rect) {
    let stats = &state.stats;
    let elapsed = stats.start_time.elapsed();

    // Progress ratio
    let completed = stats.succeeded + stats.failed;
    let ratio = if stats.total > 0 {
        completed as f64 / stats.total as f64
    } else {
        0.0
    };

    let mut lines: Vec<Line> = Vec::new();

    // Progress gauge line
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("  {}/{}", completed, stats.total),
        Style::default().fg(THEME.text).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));

    let label_style = Style::default().fg(THEME.dimmed);
    let value_style = Style::default().fg(THEME.text);

    // Status counts
    let status_items: Vec<(&str, usize, Style)> = vec![
        ("Queued", stats.queued, value_style),
        ("Running", stats.running, Style::default().fg(THEME.info)),
        (
            "Succeeded",
            stats.succeeded,
            Style::default().fg(THEME.success),
        ),
        (
            "Failed",
            stats.failed,
            if stats.failed > 0 {
                Style::default().fg(THEME.danger)
            } else {
                value_style
            },
        ),
    ];

    for (label, count, style) in &status_items {
        let pad = 12usize.saturating_sub(label.len());
        lines.push(Line::from(vec![
            Span::styled(format!("  {label}"), label_style),
            Span::raw(" ".repeat(pad)),
            Span::styled(count.to_string(), *style),
        ]));
    }

    lines.push(Line::from(""));

    // Tokens
    lines.push(Line::from(vec![Span::styled("  Tokens", label_style)]));
    lines.push(Line::from(vec![
        Span::styled("    In", label_style),
        Span::raw("      "),
        Span::styled(format_number(stats.input_tokens), value_style),
    ]));
    lines.push(Line::from(vec![
        Span::styled("    Out", label_style),
        Span::raw("     "),
        Span::styled(format_number(stats.output_tokens), value_style),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  Cost", label_style),
        Span::raw("      "),
        Span::styled(
            pricing::format_cost(stats.cost),
            Style::default().fg(THEME.info),
        ),
    ]));

    lines.push(Line::from(""));

    // Elapsed / ETA
    lines.push(Line::from(vec![
        Span::styled("  Elapsed", label_style),
        Span::raw("   "),
        Span::styled(format_duration(elapsed), value_style),
    ]));
    if let Some(eta) = stats.eta() {
        lines.push(Line::from(vec![
            Span::styled("  ETA", label_style),
            Span::raw("       "),
            Span::styled(format_duration(eta), value_style),
        ]));
    }

    // Model
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("  {}", plan.model),
        Style::default().fg(THEME.dimmed),
    )));

    let sidebar_block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(THEME.border));

    let gauge_area = Rect::new(area.x + 2, area.y + 1, area.width.saturating_sub(4), 1);

    let para = Paragraph::new(Text::from(lines)).block(sidebar_block);
    frame.render_widget(para, area);

    // Render gauge on top of sidebar
    if gauge_area.width > 0 {
        let gauge = Gauge::default()
            .ratio(ratio.min(1.0))
            .gauge_style(Style::default().fg(THEME.info).bg(THEME.highlight_bg));
        frame.render_widget(gauge, gauge_area);
    }
}

fn draw_row_table(state: &RunState, frame: &mut Frame, area: Rect) {
    let visible_height = area.height.saturating_sub(2) as usize; // header + border
    let start = state.scroll;
    let end = (start + visible_height).min(state.row_order.len());

    let header = Row::new(vec![
        Cell::from(Span::styled("ID", Style::default().fg(THEME.header))),
        Cell::from(Span::styled("Status", Style::default().fg(THEME.header))),
        Cell::from(Span::styled("Attempt", Style::default().fg(THEME.header))),
        Cell::from(Span::styled("Elapsed", Style::default().fg(THEME.header))),
    ]);

    let rows: Vec<Row> = state.row_order[start..end]
        .iter()
        .map(|&idx| {
            let row = &state.rows[idx];
            let (status_str, status_style) = match &row.state {
                RowState::Running => {
                    let frame_idx = (state.tick as usize) % SPINNER_FRAMES.len();
                    (
                        SPINNER_FRAMES[frame_idx].to_string(),
                        Style::default().fg(THEME.info),
                    )
                }
                RowState::Retrying { .. } => {
                    let frame_idx = (state.tick as usize) % SPINNER_FRAMES.len();
                    (
                        SPINNER_FRAMES[frame_idx].to_string(),
                        Style::default().fg(THEME.warning),
                    )
                }
                RowState::Succeeded => ("\u{2713}".to_string(), Style::default().fg(THEME.success)),
                RowState::Failed { .. } => {
                    ("\u{2717}".to_string(), Style::default().fg(THEME.danger))
                }
                RowState::Cancelled => ("-".to_string(), Style::default().fg(THEME.dimmed)),
            };

            let attempt_str = format!("{}/{}", row.attempt, row.max_attempts);
            let elapsed_str = if row.elapsed > Duration::ZERO {
                format_duration_short(row.elapsed)
            } else {
                String::new()
            };

            Row::new(vec![
                Cell::from(Span::styled(
                    row.id.clone(),
                    Style::default().fg(THEME.text),
                )),
                Cell::from(Span::styled(status_str, status_style)),
                Cell::from(Span::styled(attempt_str, Style::default().fg(THEME.dimmed))),
                Cell::from(Span::styled(elapsed_str, Style::default().fg(THEME.dimmed))),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Min(16),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(8),
        ],
    )
    .header(header);

    frame.render_widget(table, area);
}

fn draw_log_strip(state: &RunState, frame: &mut Frame, area: Rect) {
    let visible = area.height as usize;
    let total = state.log.len();
    let start = total.saturating_sub(visible);

    let lines: Vec<Line> = state.log[start..]
        .iter()
        .map(|l| {
            Line::from(Span::styled(
                format!(" {l}"),
                Style::default().fg(THEME.warning),
            ))
        })
        .collect();

    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(THEME.border));
    let para = Paragraph::new(Text::from(lines))
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(para, area);
}

// ---------------------------------------------------------------------------
// Done
// ---------------------------------------------------------------------------

fn draw_done(state: &DoneState, plan: &BatchPlan, frame: &mut Frame) {
    let area = frame.area();
    let summary = &state.summary;
    let has_failures = !summary.failed_rows.is_empty();

    // Keep the running layout: sidebar + row table on top, summary banner at bottom
    let bottom_height = if has_failures {
        // Need more space for failure triage
        (summary.failed_rows.len() as u16 + 6)
            .min(area.height / 2)
            .max(8)
    } else {
        5
    };

    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(bottom_height),
            Constraint::Length(1),
        ])
        .split(area);

    let body = main_chunks[0];
    let bottom_area = main_chunks[1];
    let footer_area = main_chunks[2];

    // --- Top: frozen sidebar + row table (reuse running screen layout) ---
    let body_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(24), Constraint::Min(1)])
        .split(body);

    draw_sidebar(&state.run, plan, frame, body_chunks[0]);
    draw_row_table(&state.run, frame, body_chunks[1]);

    // --- Bottom: summary banner (or failure triage) ---
    if has_failures {
        draw_failure_triage(state, frame, bottom_area);
    } else {
        draw_success_banner(summary, frame, bottom_area);
    }

    // --- Footer ---
    let footer_spans: Vec<Span> = if has_failures {
        [
            footer_cmd("r", "Retry failed"),
            vec![footer_pipe()],
            footer_cmd("j/k", "Browse"),
            vec![footer_pipe()],
            footer_cmd("q", "Quit"),
        ]
        .concat()
    } else {
        footer_cmd("q", "Quit")
    };
    frame.render_widget(Line::from(footer_spans), footer_area);
}

fn draw_success_banner(
    summary: &super::super::events::BatchSummary,
    frame: &mut Frame,
    area: Rect,
) {
    let label_style = Style::default().fg(THEME.dimmed);
    let value_style = Style::default().fg(THEME.text);

    let total_tokens = summary.input_tokens + summary.output_tokens;
    let avg_per_row = if summary.total > 0 {
        format!(
            "{:.1}s avg/row",
            summary.elapsed.as_secs_f64() / summary.total as f64
        )
    } else {
        String::new()
    };

    let banner = Line::from(vec![
        Span::styled(
            " \u{2713} Batch complete ",
            Style::default()
                .fg(THEME.success)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                " {} rows  {} tokens  {}  {} ({})",
                summary.total,
                format_number(total_tokens),
                pricing::format_cost(summary.cost),
                format_duration(summary.elapsed),
                avg_per_row,
            ),
            value_style,
        ),
    ]);

    let output_line = Line::from(vec![
        Span::styled(" Output written to ", label_style),
        Span::styled(&summary.output_path, value_style),
    ]);

    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(THEME.success));

    let text = Text::from(vec![Line::from(""), banner, output_line, Line::from("")]);

    let para = Paragraph::new(text).block(block);
    frame.render_widget(para, area);
}

fn draw_failure_triage(state: &DoneState, frame: &mut Frame, area: Rect) {
    let summary = &state.summary;
    let label_style = Style::default().fg(THEME.dimmed);

    // Split into left (failed row list) and right (detail)
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(24), Constraint::Min(1)])
        .split(area);

    let left = chunks[0];
    let right = chunks[1];

    // --- Left: failed row list ---
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled(
        format!(" \u{26a0} {} rows failed", summary.failed),
        Style::default()
            .fg(THEME.warning)
            .add_modifier(Modifier::BOLD),
    )));

    for (i, failed) in summary.failed_rows.iter().enumerate() {
        let marker = if i == state.cursor { "\u{25b8} " } else { "  " };
        let style = if i == state.cursor {
            Style::default()
                .fg(THEME.highlight_fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(THEME.text)
        };
        lines.push(Line::from(Span::styled(
            format!(" {marker}{}", failed.id),
            style,
        )));
    }

    let left_block = Block::default()
        .borders(Borders::TOP | Borders::RIGHT)
        .border_style(Style::default().fg(THEME.warning));
    let left_para = Paragraph::new(Text::from(lines)).block(left_block);
    frame.render_widget(left_para, left);

    // --- Right: detail for selected row ---
    let mut detail_lines: Vec<Line> = Vec::new();

    if let Some(failed) = summary.failed_rows.get(state.cursor) {
        detail_lines.push(Line::from(vec![
            Span::styled(
                format!(" {}", failed.id),
                Style::default().fg(THEME.text).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ", Style::default()),
            Span::styled(&failed.error, Style::default().fg(THEME.danger)),
        ]));

        // Show row fields on one line
        let fields: Vec<String> = failed
            .row_data
            .iter()
            .filter(|(k, _)| !k.starts_with('_'))
            .map(|(k, v)| {
                let val = match v {
                    serde_json::Value::String(s) if s.is_empty() => "(empty)".to_string(),
                    serde_json::Value::String(s) if s.len() > 30 => format!("{}...", &s[..30]),
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                format!("{k}: {val}")
            })
            .collect();

        if !fields.is_empty() {
            detail_lines.push(Line::from(Span::styled(
                format!(" {}", fields.join("  ")),
                label_style,
            )));
        }
    }

    let right_block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(THEME.warning));
    let right_para = Paragraph::new(Text::from(detail_lines))
        .block(right_block)
        .wrap(Wrap { trim: false });
    frame.render_widget(right_para, right);
}

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

fn draw_error(msg: &str, frame: &mut Frame) {
    let area = frame.area();

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  Error",
            Style::default()
                .fg(THEME.danger)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("  {msg}"),
            Style::default().fg(THEME.text),
        )),
        Line::from(""),
    ];

    let block = Block::bordered()
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(THEME.danger));
    let para = Paragraph::new(Text::from(lines))
        .block(block)
        .wrap(Wrap { trim: false });

    let footer_spans: Vec<Span> = footer_cmd("q", "Quit");

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);

    frame.render_widget(para, chunks[0]);
    frame.render_widget(Line::from(footer_spans), chunks[1]);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    }
}

fn format_duration_short(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        format!("{ms}ms")
    } else {
        format!("{:.1}s", d.as_secs_f64())
    }
}

fn format_number(n: u64) -> String {
    if n < 1_000 {
        n.to_string()
    } else if n < 1_000_000 {
        format!("{},{:03}", n / 1_000, n % 1_000)
    } else {
        format!(
            "{},{:03},{:03}",
            n / 1_000_000,
            (n % 1_000_000) / 1_000,
            n % 1_000
        )
    }
}

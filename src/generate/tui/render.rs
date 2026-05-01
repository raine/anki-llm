use pulldown_cmark::{Event as MarkdownEvent, Options, Parser, Tag, TagEnd};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use super::render_chrome::{draw_footer, draw_help_overlay, draw_sidebar};
use super::runner::done_audio_cache_path;
use super::screens::review::draw_reviewing;
use super::screens::selection::draw_selecting;
use super::state::{App, AppMode, summary_step_idx};
use super::widgets::{draw_log_panel, draw_model_picker, draw_step_logs};

use crate::generate::cards::ValidatedCard;
use crate::generate::pipeline::PipelineStep;
use crate::llm::pricing;
use crate::tui::line_input::LineInput;
use crate::tui::theme::THEME;

pub(super) fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();

    // Persistent shell: sidebar | main content, with footer below
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(area);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(30), Constraint::Min(0)])
        .split(rows[0]);

    // Render main content first, then sidebar on top so CJK bleed is covered
    let main_area = cols[1];
    match &app.mode {
        AppMode::Input(input) => draw_input(
            frame,
            input,
            app.history.browse_position(),
            app.batch_queue.len(),
            main_area,
            app.model_picker.is_none(),
        ),
        AppMode::Running => draw_running(frame, app, main_area),
        AppMode::Selecting(state) => draw_selecting(frame, state, &app.glyphs, app.tick, main_area),
        AppMode::Reviewing(state) => draw_reviewing(frame, state, main_area),
        AppMode::Done {
            message,
            cards,
            failed,
            ..
        } => {
            let step_idx = app.browse_step.unwrap_or_else(summary_step_idx);
            let record = &app.steps[step_idx];
            if matches!(record.step, PipelineStep::Summary) {
                draw_done(frame, app, message, cards, *failed, main_area);
            } else {
                draw_step_logs(
                    frame,
                    record.step.label(),
                    &record.logs,
                    app.browse_scroll,
                    main_area,
                );
            }
        }
        AppMode::Error(msg) => {
            let step_idx = app.browse_step.unwrap_or_else(summary_step_idx);
            let record = &app.steps[step_idx];
            if matches!(record.step, PipelineStep::Summary) {
                draw_error(frame, msg, main_area);
            } else {
                draw_step_logs(
                    frame,
                    record.step.label(),
                    &record.logs,
                    app.browse_scroll,
                    main_area,
                );
            }
        }
    }

    draw_sidebar(frame, app, cols[0]);
    draw_footer(frame, app, rows[1]);

    // Toast notification (e.g. "Copied!")
    if let Some(ref toast) = app.toast
        && app.tick.wrapping_sub(toast.tick) < 20
    {
        let text = &toast.message;
        let width = (text.len() as u16) + 2; // 1 padding each side
        let toast_area = Rect {
            x: main_area.x + 1,
            y: main_area.y + main_area.height.saturating_sub(2),
            width: width.min(main_area.width),
            height: 1,
        };
        let para = Paragraph::new(Span::styled(
            format!(" {text} "),
            Style::default()
                .fg(THEME.success)
                .add_modifier(Modifier::BOLD),
        ));
        frame.render_widget(Clear, toast_area);
        frame.render_widget(para, toast_area);
    }

    if app.show_help {
        draw_help_overlay(frame, app);
    }

    if let Some(picker) = &app.model_picker {
        draw_model_picker(frame, picker);
    }
}

fn draw_input(
    frame: &mut Frame,
    input: &LineInput,
    history_pos: Option<(usize, usize)>,
    batch_queued: usize,
    area: Rect,
    show_cursor: bool,
) {
    // Center the input box in the main area
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

    let input_block_area = Rect {
        height: 3,
        ..v_chunks[1]
    };
    let inner_width = input_block_area.width.saturating_sub(2).max(1) as usize;
    let scroll = input.visual_scroll(inner_width);

    let title = if batch_queued > 0 {
        format!(" Enter term ({} queued) ", batch_queued)
    } else {
        " Enter term ".to_string()
    };

    let mut block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(THEME.info));

    if let Some((pos, total)) = history_pos {
        let indicator = format!(" {pos}/{total} ");
        block = block.title(
            Line::from(Span::styled(indicator, Style::default().fg(THEME.dimmed))).right_aligned(),
        );
    }

    let para = Paragraph::new(input.value())
        .block(block)
        .scroll((0, scroll as u16));
    frame.render_widget(para, input_block_area);

    if show_cursor {
        frame.set_cursor_position((
            input_block_area.x + 1 + (input.visual_cursor().saturating_sub(scroll)) as u16,
            input_block_area.y + 1,
        ));
    }
}

fn draw_running(frame: &mut Frame, app: &App, area: Rect) {
    if app.thinking.is_empty() || area.height < 8 {
        draw_log_panel(frame, &app.logs, app.log_scroll, area);
        return;
    }

    let log_height = (app.logs.len() as u16 + 2).clamp(4, 10);
    let log_height = log_height.min(area.height.saturating_sub(4));
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(log_height), Constraint::Min(4)])
        .split(area);

    draw_log_panel(frame, &app.logs, app.log_scroll, chunks[0]);

    let thinking_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(0)])
        .split(chunks[1]);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "Thinking",
            Style::default()
                .fg(THEME.info)
                .add_modifier(Modifier::ITALIC),
        ))),
        thinking_chunks[0],
    );

    let content_width = thinking_chunks[1].width.max(1);
    let lines = thinking_markdown_lines(&app.thinking);
    let visual_lines = lines
        .iter()
        .map(|line| visual_line_count(line, content_width))
        .sum::<u16>();
    let scroll = visual_lines.saturating_sub(thinking_chunks[1].height);
    let paragraph = Paragraph::new(Text::from(lines))
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    frame.render_widget(paragraph, thinking_chunks[1]);
}

fn thinking_markdown_lines(md: &str) -> Vec<Line<'static>> {
    let parser = Parser::new_ext(md, Options::all());
    let base_style = Style::default()
        .fg(THEME.dimmed)
        .add_modifier(Modifier::ITALIC);
    let mut style = base_style;
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut list_depth = 0usize;

    for event in parser {
        match event {
            MarkdownEvent::Text(text) => spans.push(Span::styled(text.to_string(), style)),
            MarkdownEvent::Code(code) => spans.push(Span::styled(
                code.to_string(),
                Style::default()
                    .fg(THEME.highlight_fg)
                    .bg(THEME.highlight_bg),
            )),
            MarkdownEvent::Html(html) | MarkdownEvent::InlineHtml(html) => {
                let tag = html.trim().to_lowercase();
                match tag.as_str() {
                    "<b>" | "<strong>" => {
                        style = style.add_modifier(Modifier::BOLD).fg(THEME.text);
                    }
                    "</b>" | "</strong>" => style = base_style,
                    "<i>" | "<em>" => {
                        style = style.add_modifier(Modifier::ITALIC);
                    }
                    "</i>" | "</em>" => style = base_style,
                    "<br>" | "<br/>" | "<br />" => {
                        lines.push(Line::from(std::mem::take(&mut spans)));
                    }
                    _ => spans.push(Span::styled(html.to_string(), style)),
                }
            }
            MarkdownEvent::SoftBreak | MarkdownEvent::HardBreak => {
                lines.push(Line::from(std::mem::take(&mut spans)));
            }
            MarkdownEvent::Start(Tag::Heading { .. }) => {
                if !spans.is_empty() {
                    lines.push(Line::from(std::mem::take(&mut spans)));
                }
                style = Style::default()
                    .fg(THEME.info)
                    .add_modifier(Modifier::BOLD | Modifier::ITALIC);
            }
            MarkdownEvent::End(TagEnd::Heading(_)) => {
                lines.push(Line::from(std::mem::take(&mut spans)));
                lines.push(Line::from(""));
                style = base_style;
            }
            MarkdownEvent::Start(Tag::Strong) => {
                style = style.add_modifier(Modifier::BOLD).fg(THEME.text);
            }
            MarkdownEvent::End(TagEnd::Strong) => style = base_style,
            MarkdownEvent::Start(Tag::Emphasis) => {
                style = style.add_modifier(Modifier::ITALIC);
            }
            MarkdownEvent::End(TagEnd::Emphasis) => style = base_style,
            MarkdownEvent::Start(Tag::List(_)) => list_depth += 1,
            MarkdownEvent::End(TagEnd::List(_)) => {
                list_depth = list_depth.saturating_sub(1);
                if !spans.is_empty() {
                    lines.push(Line::from(std::mem::take(&mut spans)));
                }
                if list_depth == 0 {
                    lines.push(Line::from(""));
                }
            }
            MarkdownEvent::Start(Tag::Item) => {
                if !spans.is_empty() {
                    lines.push(Line::from(std::mem::take(&mut spans)));
                }
                spans.push(Span::styled(
                    format!("{}• ", "  ".repeat(list_depth.saturating_sub(1))),
                    base_style,
                ));
            }
            MarkdownEvent::End(TagEnd::Item) => {
                if !spans.is_empty() {
                    lines.push(Line::from(std::mem::take(&mut spans)));
                }
            }
            MarkdownEvent::End(TagEnd::Paragraph) => {
                lines.push(Line::from(std::mem::take(&mut spans)));
                if list_depth == 0 {
                    lines.push(Line::from(""));
                }
            }
            MarkdownEvent::Start(Tag::CodeBlock(_)) => {
                if !spans.is_empty() {
                    lines.push(Line::from(std::mem::take(&mut spans)));
                }
                style = Style::default()
                    .fg(THEME.highlight_fg)
                    .bg(THEME.highlight_bg);
            }
            MarkdownEvent::End(TagEnd::CodeBlock) => {
                lines.push(Line::from(std::mem::take(&mut spans)));
                style = base_style;
            }
            MarkdownEvent::Rule => lines.push(Line::from(Span::styled(
                "─".repeat(20),
                Style::default().fg(THEME.border),
            ))),
            _ => {}
        }
    }

    if !spans.is_empty() {
        lines.push(Line::from(spans));
    }
    if lines.is_empty() {
        lines.push(Line::from(""));
    }
    lines
}

fn visual_line_count(line: &Line<'_>, width: u16) -> u16 {
    let width = width.max(1) as usize;
    let cells = line
        .spans
        .iter()
        .map(|span| span.content.chars().count())
        .sum::<usize>();
    cells.div_ceil(width).max(1) as u16
}

fn draw_done(
    frame: &mut Frame,
    app: &App,
    msg: &str,
    cards: &[ValidatedCard],
    failed: bool,
    area: Rect,
) {
    let (header_text, header_color, body_color) = if failed {
        ("✗ Failed", THEME.danger, THEME.danger)
    } else {
        ("✓ Done", THEME.success, THEME.text)
    };
    let mut summary_lines = vec![
        Line::from(Span::styled(
            header_text,
            Style::default()
                .fg(header_color)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(msg, Style::default().fg(body_color))),
    ];

    if app.run_cost > 0.0 {
        summary_lines.push(Line::from(""));
        summary_lines.push(Line::from(format!(
            "Tokens: {} in / {} out  |  Cost: {}",
            app.run_input_tokens,
            app.run_output_tokens,
            pricing::format_cost(app.run_cost)
        )));
        if app.session_cost > 0.0 {
            summary_lines.push(Line::from(format!(
                "Session total: {}",
                pricing::format_cost(app.session_cost + app.run_cost)
            )));
        }
    }

    if app
        .session_info
        .as_ref()
        .map(|info| info.tts_configured)
        .unwrap_or(false)
        && app.player.is_some()
        && cards.first().and_then(done_audio_cache_path).is_some()
    {
        summary_lines.push(Line::from(""));
        summary_lines.push(Line::from(Span::styled(
            "♪ Audio ready (press p to replay)",
            Style::default().fg(THEME.success),
        )));
    }

    if cards.is_empty() {
        let para = Paragraph::new(Text::from(summary_lines)).wrap(Wrap { trim: false });
        frame.render_widget(para, area);
        return;
    }

    let summary_height = summary_lines.len() as u16 + 1; // +1 for spacing
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(summary_height), Constraint::Min(0)])
        .split(area);

    let para = Paragraph::new(Text::from(summary_lines)).wrap(Wrap { trim: false });
    frame.render_widget(para, chunks[0]);

    // Show final cards
    let mut card_lines: Vec<Line> = Vec::new();
    for (i, card) in cards.iter().enumerate() {
        if i > 0 {
            card_lines.push(Line::from(Span::styled(
                "─".repeat(40),
                Style::default().fg(THEME.border),
            )));
        }
        for (name, value) in &card.raw_anki_fields {
            card_lines.push(Line::from(Span::styled(
                name.clone(),
                Style::default().fg(THEME.info).add_modifier(Modifier::BOLD),
            )));
            card_lines.extend(crate::generate::selector::markdown_to_lines(value, "  "));
            card_lines.push(Line::from(""));
        }
    }

    let card_block = Block::default()
        .borders(Borders::ALL.difference(Borders::LEFT))
        .title(format!(" Cards ({}) ", cards.len()))
        .border_style(Style::default().fg(THEME.border));
    let card_para = Paragraph::new(Text::from(card_lines))
        .block(card_block)
        .wrap(Wrap { trim: false })
        .scroll((app.browse_scroll, 0));
    frame.render_widget(card_para, chunks[1]);
}

fn draw_error(frame: &mut Frame, msg: &str, area: Rect) {
    let lines = vec![
        Line::from(Span::styled(
            "✗ Error",
            Style::default()
                .fg(THEME.danger)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(msg, Style::default().fg(THEME.danger))),
    ];

    let para = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
    frame.render_widget(para, area);
}

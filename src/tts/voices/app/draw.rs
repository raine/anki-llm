use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};

use crate::tts::voices::catalog::VoiceEntry;
use crate::tts::voices::credentials::ProviderPreviewState;
use crate::tts::voices::yaml::emit_scaffold;
use crate::tui::theme::{THEME, footer_cmd, footer_pipe};

use super::state::{App, FilterFacet, OverlayAction, ViewState};

pub(super) fn draw(frame: &mut Frame, app: &App, view: &mut ViewState) {
    let area = frame.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(rows[0]);

    draw_list_pane(frame, cols[0], app, view);
    draw_detail_pane(frame, cols[1], app);
    draw_status(frame, rows[1], app);
    draw_footer(frame, rows[2]);

    if let Some(ref toast) = app.toast
        && app.tick.wrapping_sub(toast.tick) < 40
    {
        let text = &toast.message;
        let width = (text.chars().count() as u16) + 2;
        let area = frame.area();
        let toast_area = Rect {
            x: area.x + 1,
            y: area.y + area.height.saturating_sub(3),
            width: width.min(area.width),
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
        draw_help_overlay(frame);
    }
    if app.overlay.is_some() {
        draw_filter_overlay(frame, app, view);
    }
}

fn draw_list_pane(frame: &mut Frame, area: Rect, app: &App, view: &mut ViewState) {
    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Min(0),
        ])
        .split(area);

    let chips = Paragraph::new(render_chip_row(app));
    frame.render_widget(chips, inner[0]);

    let search_value = if app.search.value().is_empty() {
        Span::styled(
            "voice id or display name",
            Style::default().fg(THEME.dimmed),
        )
    } else {
        Span::styled(
            app.search.value().to_string(),
            Style::default().fg(THEME.text),
        )
    };
    let search_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(THEME.border))
        .title(Span::styled(
            " Text Search ",
            Style::default()
                .fg(THEME.header)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled("Ctrl+P/L/G/O/T", Style::default().fg(THEME.dimmed)),
            Span::styled(" filters ", Style::default().fg(THEME.help_muted)),
            Span::styled("Ctrl+R", Style::default().fg(THEME.dimmed)),
            Span::styled(" clear ", Style::default().fg(THEME.help_muted)),
            Span::styled("?", Style::default().fg(THEME.dimmed)),
            Span::styled(" help ", Style::default().fg(THEME.help_muted)),
        ]));
    let search_para = Paragraph::new(Line::from(vec![
        Span::styled("/ ", Style::default().fg(THEME.dimmed)),
        search_value,
    ]))
    .block(search_block);
    frame.render_widget(search_para, inner[1]);

    let items: Vec<ListItem> = app
        .filtered
        .iter()
        .map(|i| render_list_row(&app.entries[*i]))
        .collect();
    let list_title = if app.filters.active_count() == 0 {
        format!(" Voices ({}) ", app.filtered.len())
    } else {
        format!(
            " Voices ({}, {} active) ",
            app.filtered.len(),
            app.filters.active_count()
        )
    };
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(THEME.border))
                .title(Span::styled(
                    list_title,
                    Style::default()
                        .fg(THEME.header)
                        .add_modifier(Modifier::BOLD),
                )),
        )
        .highlight_style(
            Style::default()
                .bg(THEME.highlight_bg)
                .fg(THEME.highlight_fg)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");
    frame.render_stateful_widget(list, inner[2], &mut view.list_state);
}

fn render_chip_row(app: &App) -> Line<'static> {
    let mut spans = Vec::new();
    let chips = [
        (
            "Provider",
            app.filters
                .provider
                .map(|provider| provider.as_str().to_string()),
            THEME.info,
        ),
        ("Lang", app.filters.language.clone(), THEME.success),
        ("Gender", app.filters.gender.clone(), THEME.warning),
        ("Engine", app.filters.engine.clone(), THEME.info),
        (
            "Tags",
            if app.filters.tags.is_empty() {
                None
            } else {
                Some(app.filters.tags.join(","))
            },
            THEME.success,
        ),
    ];
    for (idx, (label, value, color)) in chips.into_iter().enumerate() {
        if idx > 0 {
            spans.push(Span::styled("  ", Style::default()));
        }
        spans.push(Span::styled(
            format!("{label}: "),
            Style::default().fg(THEME.dimmed),
        ));
        match value {
            Some(value) => spans.push(Span::styled(
                value,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            )),
            None => spans.push(Span::styled("any", Style::default().fg(THEME.dimmed))),
        }
    }
    Line::from(spans)
}

fn render_list_row(entry: &VoiceEntry) -> ListItem<'static> {
    let provider = format!("{:<7}", entry.provider.as_str());
    let lang = entry
        .languages
        .first()
        .map(String::as_str)
        .unwrap_or(if entry.multilingual { "*" } else { "--" });
    let gender = entry.gender.as_deref().unwrap_or("-");
    let line = Line::from(vec![
        Span::styled(provider, Style::default().fg(THEME.info)),
        Span::raw(" "),
        Span::styled(format!("{:<8}", lang), Style::default().fg(THEME.dimmed)),
        Span::raw(" "),
        Span::styled(format!("{:<6}", gender), Style::default().fg(THEME.dimmed)),
        Span::raw(" "),
        Span::styled(
            entry.voice_id.clone(),
            Style::default().fg(THEME.text).add_modifier(Modifier::BOLD),
        ),
    ]);
    ListItem::new(line)
}

fn draw_detail_pane(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(THEME.border))
        .title(Span::styled(
            " Details ",
            Style::default()
                .fg(THEME.header)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(entry) = app.selected_entry() else {
        let empty = Paragraph::new(Span::styled(
            "No voices match current filters.",
            Style::default().fg(THEME.dimmed),
        ))
        .wrap(Wrap { trim: false });
        frame.render_widget(empty, inner);
        return;
    };

    let state = app.provider_states.get(&entry.provider);
    let status_line = match state {
        Some(ProviderPreviewState::Ready { .. }) => {
            Span::styled("Ready", Style::default().fg(THEME.success))
        }
        Some(ProviderPreviewState::Unavailable { reason }) => Span::styled(
            format!("Unavailable · {reason}"),
            Style::default().fg(THEME.warning),
        ),
        None => Span::styled("Unknown", Style::default().fg(THEME.dimmed)),
    };

    let languages = if entry.multilingual {
        "multilingual".to_string()
    } else if entry.languages.is_empty() {
        "--".into()
    } else {
        entry.languages.join(", ")
    };

    let region = app.region_for(entry);
    let yaml = emit_scaffold(entry, region.as_deref());

    let mut lines: Vec<Line<'static>> = vec![
        kv("Provider", entry.provider.as_str().to_string()),
        kv("Voice", entry.voice_id.clone()),
        kv("Name", entry.display_name.clone()),
        kv("Languages", languages),
        kv(
            "Gender",
            entry.gender.clone().unwrap_or_else(|| "--".into()),
        ),
    ];
    if let Some(m) = &entry.preview_model {
        lines.push(kv("Engine", m.clone()));
    }
    if !entry.tags.is_empty() {
        lines.push(kv("Tags", entry.tags.join(", ")));
    }
    lines.push(Line::from(vec![
        Span::styled("Status: ", Style::default().fg(THEME.dimmed)),
        status_line,
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "YAML scaffold",
        Style::default()
            .fg(THEME.header)
            .add_modifier(Modifier::BOLD),
    )));
    for y_line in yaml.lines() {
        lines.push(Line::from(Span::styled(
            y_line.to_string(),
            Style::default().fg(THEME.text),
        )));
    }

    let para = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(para, inner);
}

fn kv(key: &str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{key}: "), Style::default().fg(THEME.dimmed)),
        Span::styled(value, Style::default().fg(THEME.text)),
    ])
}

fn draw_status(frame: &mut Frame, area: Rect, app: &App) {
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            app.status_line.clone(),
            Style::default().fg(THEME.info),
        ))),
        area,
    );
}

fn draw_footer(frame: &mut Frame, area: Rect) {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (idx, (key, label)) in [
        ("↑↓", "select"),
        ("type", "text"),
        ("Ctrl+P/L/G/O/T", "filter"),
        ("Space", "preview"),
        ("Enter", "copy yaml"),
        ("Esc", "cancel"),
    ]
    .iter()
    .enumerate()
    {
        if idx > 0 {
            spans.push(footer_pipe());
        }
        spans.extend(footer_cmd(key, label));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_help_overlay(frame: &mut Frame) {
    let area = centered_rect(frame.area(), 62, 12);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(THEME.help_border))
        .title(Span::styled(
            " Help ",
            Style::default()
                .fg(THEME.header)
                .add_modifier(Modifier::BOLD),
        ));
    let lines = vec![
        Line::from(vec![
            Span::styled("Type", Style::default().fg(THEME.dimmed)),
            Span::styled(
                " to search voice id and display name",
                Style::default().fg(THEME.text),
            ),
        ]),
        Line::from(vec![
            Span::styled("Ctrl+P", Style::default().fg(THEME.dimmed)),
            Span::styled(" provider filter", Style::default().fg(THEME.text)),
        ]),
        Line::from(vec![
            Span::styled("Ctrl+L", Style::default().fg(THEME.dimmed)),
            Span::styled(" language filter", Style::default().fg(THEME.text)),
        ]),
        Line::from(vec![
            Span::styled("Ctrl+G", Style::default().fg(THEME.dimmed)),
            Span::styled(" gender filter", Style::default().fg(THEME.text)),
        ]),
        Line::from(vec![
            Span::styled("Ctrl+O", Style::default().fg(THEME.dimmed)),
            Span::styled(" engine filter", Style::default().fg(THEME.text)),
        ]),
        Line::from(vec![
            Span::styled("Ctrl+T", Style::default().fg(THEME.dimmed)),
            Span::styled(
                " tag filter (Space toggles)",
                Style::default().fg(THEME.text),
            ),
        ]),
        Line::from(vec![
            Span::styled("Ctrl+R", Style::default().fg(THEME.dimmed)),
            Span::styled(" clear all filters", Style::default().fg(THEME.text)),
        ]),
        Line::from(vec![
            Span::styled("Any key", Style::default().fg(THEME.dimmed)),
            Span::styled(" closes this help", Style::default().fg(THEME.text)),
        ]),
    ];
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_filter_overlay(frame: &mut Frame, app: &App, view: &mut ViewState) {
    let Some(overlay) = app.overlay.as_ref() else {
        return;
    };
    let facet = overlay.facet;
    let rows = app.overlay_rows_for(facet, overlay.search.value());

    let area = centered_rect(
        frame.area(),
        if matches!(facet, FilterFacet::Language | FilterFacet::Tag) {
            64
        } else {
            52
        },
        18,
    );
    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(area);

    let search_value = if overlay.search.value().is_empty() {
        Span::styled("type to narrow", Style::default().fg(THEME.dimmed))
    } else {
        Span::styled(
            overlay.search.value().to_string(),
            Style::default().fg(THEME.text),
        )
    };
    let search_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(THEME.help_border))
        .title(Span::styled(
            format!(" {} ", overlay.facet.title()),
            Style::default()
                .fg(THEME.header)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled("Enter", Style::default().fg(THEME.dimmed)),
            Span::styled(" apply ", Style::default().fg(THEME.help_muted)),
            Span::styled("Esc", Style::default().fg(THEME.dimmed)),
            Span::styled(" close ", Style::default().fg(THEME.help_muted)),
            Span::styled("↑↓", Style::default().fg(THEME.dimmed)),
            Span::styled(" move ", Style::default().fg(THEME.help_muted)),
            Span::styled(
                if facet.multi_select() {
                    "Space"
                } else {
                    facet.key_hint()
                },
                Style::default().fg(THEME.dimmed),
            ),
            Span::styled(
                if facet.multi_select() {
                    " toggle "
                } else {
                    " facet "
                },
                Style::default().fg(THEME.help_muted),
            ),
        ]));
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("/ ", Style::default().fg(THEME.dimmed)),
            search_value,
        ]))
        .block(search_block),
        parts[0],
    );

    let items: Vec<ListItem> = rows
        .into_iter()
        .map(|row| {
            let marker = if facet.multi_select() && !matches!(row.action, OverlayAction::ClearTags)
            {
                if row.selected { "[x] " } else { "[ ] " }
            } else if row.selected {
                "* "
            } else {
                "  "
            };
            let style = if row.selected {
                Style::default().fg(THEME.success)
            } else {
                Style::default().fg(THEME.text)
            };
            ListItem::new(Line::from(vec![
                Span::styled(marker, Style::default().fg(THEME.dimmed)),
                Span::styled(row.label, style),
                Span::raw(" "),
                Span::styled(
                    format!("({})", row.count),
                    Style::default().fg(THEME.dimmed),
                ),
            ]))
        })
        .collect();
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(THEME.help_border)),
        )
        .highlight_style(
            Style::default()
                .bg(THEME.highlight_bg)
                .fg(THEME.highlight_fg)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");
    frame.render_stateful_widget(list, parts[1], &mut view.overlay_list_state);
}

fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    Rect::new(
        area.x + area.width.saturating_sub(width.min(area.width)) / 2,
        area.y + area.height.saturating_sub(height.min(area.height)) / 2,
        width.min(area.width),
        height.min(area.height),
    )
}

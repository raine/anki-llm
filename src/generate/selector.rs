use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::cards::ValidatedCard;

pub fn strip_html_tags(html: &str) -> String {
    // Simple regex-free HTML stripping
    let mut result = String::new();
    let mut in_tag = false;
    let mut need_space = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                need_space = true;
            }
            _ if !in_tag => {
                if need_space && !ch.is_ascii_punctuation() && !result.is_empty() {
                    result.push(' ');
                }
                need_space = false;
                result.push(ch);
            }
            _ => {}
        }
    }
    // Clean up whitespace
    result.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Render markdown to a string with ANSI escape codes for terminal display.
pub fn markdown_to_ansi(md: &str) -> String {
    const BOLD: &str = "\x1b[1m";
    const ITALIC: &str = "\x1b[3m";
    const CODE: &str = "\x1b[7m"; // reverse video for inline code
    const RESET: &str = "\x1b[0m";

    let mut out = String::new();
    let parser = Parser::new_ext(md, Options::all());

    for event in parser {
        match event {
            Event::Text(t) => out.push_str(&t),
            Event::Code(t) => {
                out.push_str(CODE);
                out.push_str(&t);
                out.push_str(RESET);
            }
            Event::Start(Tag::Strong) => out.push_str(BOLD),
            Event::End(TagEnd::Strong) => out.push_str(RESET),
            Event::Start(Tag::Emphasis) => out.push_str(ITALIC),
            Event::End(TagEnd::Emphasis) => out.push_str(RESET),
            Event::Start(Tag::Item) => out.push_str("• "),
            Event::SoftBreak | Event::HardBreak => out.push('\n'),
            Event::End(TagEnd::Paragraph) | Event::End(TagEnd::Item) => out.push('\n'),
            _ => {}
        }
    }

    out.trim_end_matches('\n').to_string()
}

/// Render markdown to ratatui `Line`s with inline styling (bold, italic, code).
pub fn markdown_to_lines(md: &str, indent: &str) -> Vec<Line<'static>> {
    let parser = Parser::new_ext(md, Options::all());
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut style = Style::default();

    // Start each line with indent
    if !indent.is_empty() {
        spans.push(Span::raw(indent.to_string()));
    }

    for event in parser {
        match event {
            Event::Text(t) => {
                spans.push(Span::styled(t.to_string(), style));
            }
            Event::Code(t) => {
                let code_style = Style::default().add_modifier(Modifier::REVERSED);
                spans.push(Span::styled(t.to_string(), code_style));
            }
            Event::Start(Tag::Strong) => style = style.add_modifier(Modifier::BOLD),
            Event::End(TagEnd::Strong) => style = style.remove_modifier(Modifier::BOLD),
            Event::Start(Tag::Emphasis) => style = style.add_modifier(Modifier::ITALIC),
            Event::End(TagEnd::Emphasis) => style = style.remove_modifier(Modifier::ITALIC),
            Event::Start(Tag::Item) => {
                spans.push(Span::raw("• ".to_string()));
            }
            Event::SoftBreak | Event::HardBreak => {
                lines.push(Line::from(spans.drain(..).collect::<Vec<_>>()));
                if !indent.is_empty() {
                    spans.push(Span::raw(indent.to_string()));
                }
            }
            Event::End(TagEnd::Paragraph) | Event::End(TagEnd::Item) => {
                lines.push(Line::from(spans.drain(..).collect::<Vec<_>>()));
                if !indent.is_empty() {
                    spans.push(Span::raw(indent.to_string()));
                }
            }
            _ => {}
        }
    }

    if !spans.is_empty() {
        // Don't push a line that's only indent whitespace
        let only_indent =
            spans.len() == 1 && spans[0].content.chars().all(|c| c.is_ascii_whitespace());
        if !only_indent {
            lines.push(Line::from(spans));
        }
    }

    lines
}

/// One-line summary of a card for list display.
pub fn format_card_summary(card: &ValidatedCard, index: usize) -> String {
    let header = if card.is_duplicate {
        format!("Card {} (Duplicate)", index + 1)
    } else {
        format!("Card {}", index + 1)
    };

    let first_field = card.anki_fields.values().next().map(|v| {
        let plain = strip_html_tags(v);
        if plain.len() > 50 {
            let boundary = plain
                .char_indices()
                .map(|(i, _)| i)
                .take_while(|&i| i <= 47)
                .last()
                .unwrap_or(0);
            format!("{}...", &plain[..boundary])
        } else {
            plain
        }
    });

    match first_field {
        Some(f) if !f.is_empty() => format!("{header}: {f}"),
        _ => header,
    }
}

/// Display cards for dry-run mode (no interaction).
pub fn display_cards(cards: &[ValidatedCard]) {
    if cards.is_empty() {
        eprintln!("\nNo cards generated.");
        return;
    }

    eprintln!("\nGenerated {} card(s):\n", cards.len());
    eprintln!("{}", "─".repeat(60));

    for (i, card) in cards.iter().enumerate() {
        let header = if card.is_duplicate {
            format!("\nCard {} (Duplicate - already exists in Anki)", i + 1)
        } else {
            format!("\nCard {}", i + 1)
        };
        eprintln!("{header}");

        for (name, value) in &card.raw_anki_fields {
            eprintln!("\n{name}:");
            eprintln!("{}", markdown_to_ansi(value));
        }

        eprintln!("\n{}", "─".repeat(60));
    }

    let dup_count = cards.iter().filter(|c| c.is_duplicate).count();
    if dup_count > 0 {
        eprintln!("\n{dup_count} card(s) are duplicates (already exist in Anki)");
    }

    eprintln!("\nDry run complete. No cards were imported or exported.");
    eprintln!("Run without --dry-run to add cards interactively.");
}

/// Interactive card selection using inquire (legacy/non-TTY path).
pub fn select_cards_legacy(cards: &[ValidatedCard]) -> Result<Vec<usize>, anyhow::Error> {
    if cards.is_empty() {
        anyhow::bail!("No cards to select from");
    }

    let options: Vec<String> = cards
        .iter()
        .enumerate()
        .map(|(i, card)| format_card_summary(card, i))
        .collect();

    eprintln!("\nSelect cards to add to Anki:\n");

    let selected = inquire::MultiSelect::new("Choose cards to import:", options.clone())
        .with_page_size(15)
        .prompt()?;

    if selected.is_empty() {
        anyhow::bail!("No cards selected");
    }

    let indices: Vec<usize> = selected
        .iter()
        .filter_map(|s| options.iter().position(|o| o == s))
        .collect();

    Ok(indices)
}

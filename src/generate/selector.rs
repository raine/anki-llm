use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

use super::cards::ValidatedCard;

fn strip_html_tags(html: &str) -> String {
    // Simple regex-free HTML stripping
    let mut result = String::new();
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }
    // Clean up whitespace
    result.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Render markdown to a string with ANSI escape codes for terminal display.
fn markdown_to_ansi(md: &str) -> String {
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

fn format_card_for_display(card: &ValidatedCard, index: usize) -> String {
    let mut lines = Vec::new();
    let header = if card.is_duplicate {
        format!("Card {} (Duplicate)", index + 1)
    } else {
        format!("Card {}", index + 1)
    };
    lines.push(header);

    for (name, value) in &card.anki_fields {
        let plain = strip_html_tags(value);
        lines.push(format!("  {name}: {plain}"));
    }

    lines.join("\n")
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

/// Interactive card selection. Returns indices of selected cards.
pub fn select_cards(cards: &[ValidatedCard]) -> Result<Vec<usize>, anyhow::Error> {
    if cards.is_empty() {
        anyhow::bail!("No cards to select from");
    }

    let options: Vec<String> = cards
        .iter()
        .enumerate()
        .map(|(i, card)| format_card_for_display(card, i))
        .collect();

    eprintln!("\nSelect cards to add to Anki:\n");

    let selected = inquire::MultiSelect::new("Choose cards to import:", options)
        .with_page_size(15)
        .prompt()?;

    if selected.is_empty() {
        anyhow::bail!("No cards selected");
    }

    // Map selected display strings back to indices
    let all_options: Vec<String> = cards
        .iter()
        .enumerate()
        .map(|(i, card)| format_card_for_display(card, i))
        .collect();

    let indices: Vec<usize> = selected
        .iter()
        .filter_map(|s| all_options.iter().position(|o| o == s))
        .collect();

    Ok(indices)
}

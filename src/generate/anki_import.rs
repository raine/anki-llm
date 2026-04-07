use crate::anki::client::AnkiClient;
use crate::anki::schema::AddNoteParams;
use crate::style::style;
use crate::template::frontmatter::Frontmatter;

use super::cards::ValidatedCard;

pub struct ImportResult {
    pub successes: usize,
    pub failures: usize,
}

/// Add cards to Anki as new notes.
pub fn import_cards_to_anki(
    cards: &[ValidatedCard],
    frontmatter: &Frontmatter,
    anki: &AnkiClient,
    on_log: &dyn Fn(&str),
) -> Result<ImportResult, anyhow::Error> {
    if cards.is_empty() {
        return Ok(ImportResult {
            successes: 0,
            failures: 0,
        });
    }

    on_log(&format!("Adding {} card(s) to Anki...", cards.len()));

    let notes: Vec<AddNoteParams> = cards
        .iter()
        .map(|card| AddNoteParams {
            deck_name: frontmatter.deck.clone(),
            model_name: frontmatter.note_type.clone(),
            fields: card.anki_fields.clone(),
            tags: vec!["anki-llm-generate".into()],
        })
        .collect();

    let results = anki.add_notes(&notes)?;
    let successes = results.iter().filter(|r| r.is_some()).count();
    let failures = results.len() - successes;

    Ok(ImportResult {
        successes,
        failures,
    })
}

/// Print import results to stderr (legacy mode).
pub fn report_import_result(result: &ImportResult, deck_name: &str) {
    let s = style();
    if result.failures > 0 {
        eprintln!(
            "\n  {} card(s) added, {} failed",
            result.successes,
            s.error_text(result.failures)
        );
        eprintln!(
            "  {}",
            s.muted("Some cards may have been duplicates or had invalid field values.")
        );
    } else if result.successes > 0 {
        eprintln!(
            "\n  {} {} note(s) to {}",
            s.success("Added"),
            result.successes,
            s.cyan(format!("\"{deck_name}\""))
        );
    } else {
        eprintln!("\n  No cards were added to Anki.");
    }
}

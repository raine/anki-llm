use crate::anki::client::AnkiClient;
use crate::template::frontmatter::Frontmatter;

pub struct ValidationResult {
    pub note_type_fields: Vec<String>,
}

/// Validate that deck, note type, and field mappings exist in Anki.
pub fn validate_anki_assets(
    anki: &AnkiClient,
    frontmatter: &Frontmatter,
) -> Result<ValidationResult, anyhow::Error> {
    // Check deck exists
    let decks = anki.deck_names()?;
    if !decks.contains(&frontmatter.deck) {
        anyhow::bail!(
            "Deck \"{}\" does not exist in Anki. Available: {}",
            frontmatter.deck,
            decks.join(", ")
        );
    }

    // Check note type and get fields
    let note_type_fields = anki
        .model_field_names(&frontmatter.note_type)
        .map_err(|_| {
            // Get available model names for error message
            let models = anki.model_names().unwrap_or_default();
            anyhow::anyhow!(
                "Note type \"{}\" does not exist. Available: {}",
                frontmatter.note_type,
                models.join(", ")
            )
        })?;

    // Validate fieldMap values exist in note type
    let invalid: Vec<_> = frontmatter
        .field_map
        .values()
        .filter(|v| !note_type_fields.contains(v))
        .collect();

    if !invalid.is_empty() {
        anyhow::bail!(
            "Fields not in note type \"{}\": {}",
            frontmatter.note_type,
            invalid
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    Ok(ValidationResult { note_type_fields })
}

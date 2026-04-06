use std::collections::HashMap;

use indexmap::IndexMap;

use crate::anki::client::AnkiClient;
use crate::template::frontmatter::Frontmatter;

use super::processor::CardCandidate;

/// A card with mapped Anki fields and duplicate status.
#[derive(Clone)]
pub struct ValidatedCard {
    /// Original fields (LLM keys → values as strings after sanitization).
    pub fields: HashMap<String, String>,
    /// Fields mapped to Anki field names (sanitized HTML, for Anki import).
    pub anki_fields: IndexMap<String, String>,
    /// Fields mapped to Anki field names (raw markdown, for terminal display).
    pub raw_anki_fields: IndexMap<String, String>,
    /// Whether this card already exists in Anki.
    pub is_duplicate: bool,
}

/// Map card fields from LLM keys to Anki field names.
pub fn map_fields_to_anki(
    sanitized: &HashMap<String, String>,
    field_map: &IndexMap<String, String>,
) -> Result<IndexMap<String, String>, anyhow::Error> {
    let mut anki_fields = IndexMap::new();
    for (llm_key, anki_name) in field_map {
        let value = sanitized
            .get(llm_key)
            .ok_or_else(|| anyhow::anyhow!("Missing field \"{llm_key}\" in card"))?;
        anki_fields.insert(anki_name.clone(), value.clone());
    }
    Ok(anki_fields)
}

/// Escape special characters for Anki search queries.
fn escape_anki_query(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('*', "\\*")
        .replace('_', "\\_")
}

/// Check if a note with this first field value already exists.
fn check_duplicate(
    anki: &AnkiClient,
    first_field_value: &str,
    note_type: &str,
    deck: &str,
) -> Result<bool, anyhow::Error> {
    let escaped = escape_anki_query(first_field_value);
    let query = format!("\"note:{note_type}\" \"deck:{deck}\" \"{escaped}\"");
    let ids = anki.find_notes(&query)?;
    Ok(!ids.is_empty())
}

/// Validate cards: map fields to Anki names and check for duplicates.
pub fn validate_cards(
    cards: Vec<(CardCandidate, HashMap<String, String>)>,
    frontmatter: &Frontmatter,
    first_field_name: &str,
    anki: &AnkiClient,
) -> Result<Vec<ValidatedCard>, anyhow::Error> {
    let mut validated = Vec::new();
    for (candidate, sanitized) in cards {
        let anki_fields = map_fields_to_anki(&sanitized, &frontmatter.field_map)?;

        // Build raw (pre-sanitization) field strings for terminal display.
        let raw_strings: HashMap<String, String> = candidate
            .fields
            .iter()
            .map(|(k, v)| {
                let s = match v {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                (k.clone(), s)
            })
            .collect();
        let raw_anki_fields = map_fields_to_anki(&raw_strings, &frontmatter.field_map)?;

        let is_duplicate = anki_fields
            .get(first_field_name)
            .filter(|v| !v.is_empty())
            .map(|v| check_duplicate(anki, v, &frontmatter.note_type, &frontmatter.deck))
            .unwrap_or(Ok(false))?;

        validated.push(ValidatedCard {
            fields: sanitized,
            anki_fields,
            raw_anki_fields,
            is_duplicate,
        });
    }
    Ok(validated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_special_chars() {
        assert_eq!(
            escape_anki_query(r#"hello "world" foo_bar"#),
            r#"hello \"world\" foo\_bar"#
        );
    }

    #[test]
    fn map_fields() {
        let mut sanitized = HashMap::new();
        sanitized.insert("front".into(), "hello".into());
        sanitized.insert("back".into(), "world".into());

        let mut field_map = IndexMap::new();
        field_map.insert("front".into(), "Front".into());
        field_map.insert("back".into(), "Back".into());

        let result = map_fields_to_anki(&sanitized, &field_map).unwrap();
        assert_eq!(result["Front"], "hello");
        assert_eq!(result["Back"], "world");
    }
}

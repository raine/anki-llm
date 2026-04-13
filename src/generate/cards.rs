use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use indexmap::IndexMap;

use crate::anki::client::AnkiClient;
use crate::template::frontmatter::Frontmatter;

use super::processor::CardCandidate;

/// Monotonic counter minting stable `ValidatedCard` ids. Used by the
/// TUI to route async per-card state (TTS preview, future edits) across
/// regeneration without relying on fragile selection indices.
static CARD_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

pub fn next_card_id() -> u64 {
    CARD_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// A card with mapped Anki fields and duplicate status.
#[derive(Clone)]
pub struct ValidatedCard {
    /// Stable identifier for per-card async state routing (TTS preview
    /// status, etc.). Re-minted on regeneration so old state for the
    /// replaced card is implicitly invalidated.
    pub card_id: u64,
    /// Original fields (LLM keys → values as strings after sanitization).
    pub fields: HashMap<String, String>,
    /// Fields mapped to Anki field names (sanitized HTML, for Anki import).
    pub anki_fields: IndexMap<String, String>,
    /// Fields mapped to Anki field names (raw markdown, for terminal display).
    pub raw_anki_fields: IndexMap<String, String>,
    /// Whether this card already exists in Anki.
    pub is_duplicate: bool,
    /// Note ID of the existing duplicate in Anki, if any.
    pub duplicate_note_id: Option<i64>,
    /// Fields of the existing duplicate note in Anki (field name → value).
    pub duplicate_fields: Option<IndexMap<String, String>>,
    /// Informational flags from pre-select check steps.
    pub flags: Vec<String>,
    /// LLM model used to generate this card.
    pub model: String,
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
pub fn escape_anki_query(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('*', "\\*")
        .replace('_', "\\_")
}

/// Check if a note with this first field value already exists (public wrapper).
pub fn check_duplicate_pub(
    anki: &AnkiClient,
    first_field_value: &str,
    note_type: &str,
    deck: &str,
) -> Result<bool, anyhow::Error> {
    Ok(check_duplicate(anki, first_field_value, note_type, deck)?.is_some())
}

/// Check if a note with this first field value already exists.
/// Returns the note ID of the first match, or None if no duplicate found.
fn check_duplicate(
    anki: &AnkiClient,
    first_field_value: &str,
    note_type: &str,
    deck: &str,
) -> Result<Option<i64>, anyhow::Error> {
    let escaped = escape_anki_query(first_field_value);
    let query = format!("\"note:{note_type}\" \"deck:{deck}\" \"{escaped}\"");
    let ids = anki.find_notes(&query)?;
    Ok(ids.into_iter().next())
}

/// Fetch the fields of an existing note by its ID.
fn fetch_note_fields(
    anki: &AnkiClient,
    note_id: i64,
) -> Result<IndexMap<String, String>, anyhow::Error> {
    let notes = anki.notes_info(&[note_id])?;
    let note = notes
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("Note {note_id} not found"))?;
    Ok(note.fields.into_iter().map(|(k, v)| (k, v.value)).collect())
}

/// Validate cards: map fields to Anki names and check for duplicates.
pub fn validate_cards(
    cards: Vec<(CardCandidate, HashMap<String, String>)>,
    frontmatter: &Frontmatter,
    first_field_name: &str,
    anki: &AnkiClient,
    model: &str,
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

        let dup_note_id = anki_fields
            .get(first_field_name)
            .filter(|v| !v.is_empty())
            .map(|v| check_duplicate(anki, v, &frontmatter.note_type, &frontmatter.deck))
            .unwrap_or(Ok(None))?;

        let duplicate_fields = if let Some(note_id) = dup_note_id {
            fetch_note_fields(anki, note_id).ok()
        } else {
            None
        };

        validated.push(ValidatedCard {
            card_id: next_card_id(),
            fields: sanitized,
            anki_fields,
            raw_anki_fields,
            is_duplicate: dup_note_id.is_some(),
            duplicate_note_id: dup_note_id,
            duplicate_fields,
            flags: Vec::new(),
            model: model.to_string(),
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

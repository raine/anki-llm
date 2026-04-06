use indexmap::IndexMap;
use serde_json::Value;

/// A single row of data. Keys are field names, values are JSON values.
/// Uses IndexMap to preserve field order from source files.
pub type Row = IndexMap<String, Value>;

/// Extract a note ID from a row, checking noteId/id/Id fields.
/// Always normalizes to a String to avoid key mismatches.
#[allow(dead_code)]
pub fn get_note_id(row: &Row) -> Option<String> {
    let value = row
        .get("noteId")
        .or_else(|| row.get("id"))
        .or_else(|| row.get("Id"))?;

    match value {
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

/// Like `get_note_id` but returns an error if no ID is found.
#[allow(dead_code)]
pub fn require_note_id(row: &Row) -> Result<String, super::error::DataError> {
    get_note_id(row).ok_or_else(|| {
        let fields: Vec<&str> = row.keys().map(|k| k.as_str()).collect();
        super::error::DataError::MissingNoteId(fields.join(", "))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn note_id_from_number() {
        let mut row = Row::new();
        row.insert("noteId".into(), json!(123));
        assert_eq!(get_note_id(&row), Some("123".to_string()));
    }

    #[test]
    fn note_id_from_string() {
        let mut row = Row::new();
        row.insert("noteId".into(), json!("123"));
        assert_eq!(get_note_id(&row), Some("123".to_string()));
    }

    #[test]
    fn note_id_fallback_to_id() {
        let mut row = Row::new();
        row.insert("id".into(), json!(456));
        assert_eq!(get_note_id(&row), Some("456".to_string()));
    }

    #[test]
    fn note_id_missing() {
        let mut row = Row::new();
        row.insert("foo".into(), json!("bar"));
        assert_eq!(get_note_id(&row), None);
    }

    #[test]
    fn require_note_id_error_lists_fields() {
        let mut row = Row::new();
        row.insert("front".into(), json!("hello"));
        row.insert("back".into(), json!("world"));
        let err = require_note_id(&row).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("front"));
        assert!(msg.contains("back"));
    }
}

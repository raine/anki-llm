use super::error::DataError;
use super::rows::Row;

/// Parse YAML content (array of objects) into rows.
pub fn parse_yaml(content: &str) -> Result<Vec<Row>, DataError> {
    let rows: Vec<Row> = serde_yaml::from_str(content)?;
    Ok(rows)
}

/// Serialize rows to YAML string.
pub fn serialize_yaml(rows: &[Row]) -> Result<String, DataError> {
    let yaml = serde_yaml::to_string(rows)?;
    Ok(yaml)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    #[test]
    fn parse_simple_yaml() {
        let content = "- front: hello\n  back: world\n- front: foo\n  back: bar\n";
        let rows = parse_yaml(content).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["front"], json!("hello"));
        assert_eq!(rows[1]["back"], json!("bar"));
    }

    #[test]
    fn parse_non_array_returns_error() {
        let content = "front: hello\nback: world\n";
        assert!(parse_yaml(content).is_err());
    }

    #[test]
    fn numeric_values_preserved() {
        let content = "- noteId: 123\n  front: hello\n";
        let rows = parse_yaml(content).unwrap();
        assert!(matches!(rows[0]["noteId"], Value::Number(_)));
        assert_eq!(rows[0]["noteId"], json!(123));
    }

    #[test]
    fn round_trip_preserves_order_and_values() {
        let mut row = Row::new();
        row.insert("noteId".into(), json!(123));
        row.insert("front".into(), json!("hello"));
        row.insert("back".into(), json!("world"));

        let yaml = serialize_yaml(&[row]).unwrap();
        let parsed = parse_yaml(&yaml).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0]["noteId"], json!(123));
        assert_eq!(parsed[0]["front"], json!("hello"));
        // Check field order is preserved
        let keys: Vec<&str> = parsed[0].keys().map(|k| k.as_str()).collect();
        assert_eq!(keys, vec!["noteId", "front", "back"]);
    }
}

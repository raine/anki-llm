use indexmap::IndexSet;
use serde_json::Value;

use super::error::DataError;
use super::rows::Row;

/// Parse CSV content (with headers) into rows.
pub fn parse_csv(content: &str) -> Result<Vec<Row>, DataError> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .from_reader(content.as_bytes());

    let headers: Vec<String> = reader
        .headers()
        .map_err(|e| DataError::CsvParse(e.to_string()))?
        .iter()
        .map(|h| h.to_string())
        .collect();

    let mut rows = Vec::new();
    for result in reader.records() {
        let record = result.map_err(|e| DataError::CsvParse(e.to_string()))?;
        let mut row = Row::new();
        for (i, field) in record.iter().enumerate() {
            if let Some(header) = headers.get(i) {
                row.insert(header.clone(), Value::String(field.to_string()));
            }
        }
        rows.push(row);
    }
    Ok(rows)
}

/// Serialize rows to CSV with all fields quoted.
pub fn serialize_csv(rows: &[Row]) -> Result<String, DataError> {
    if rows.is_empty() {
        return Ok(String::new());
    }

    // Collect all unique headers preserving order from first row,
    // then appending any new keys from subsequent rows.
    let mut headers = IndexSet::new();
    for row in rows {
        for key in row.keys() {
            headers.insert(key.clone());
        }
    }

    let mut writer = csv::WriterBuilder::new()
        .quote_style(csv::QuoteStyle::Always)
        .terminator(csv::Terminator::Any(b'\n'))
        .from_writer(Vec::new());

    writer
        .write_record(&headers)
        .map_err(|e| DataError::CsvParse(e.to_string()))?;

    for row in rows {
        let record: Vec<String> = headers
            .iter()
            .map(|h| match row.get(h) {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Number(n)) => n.to_string(),
                Some(Value::Null) => String::new(),
                Some(v) => v.to_string(),
                None => String::new(),
            })
            .collect();
        writer
            .write_record(&record)
            .map_err(|e| DataError::CsvParse(e.to_string()))?;
    }

    let bytes = writer
        .into_inner()
        .map_err(|e| DataError::CsvParse(e.to_string()))?;
    String::from_utf8(bytes).map_err(|e| DataError::CsvParse(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_simple_csv() {
        let content = "front,back\nhello,world\nfoo,bar\n";
        let rows = parse_csv(content).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["front"], json!("hello"));
        assert_eq!(rows[0]["back"], json!("world"));
    }

    #[test]
    fn empty_input_returns_empty_vec() {
        let rows = parse_csv("front,back\n").unwrap();
        assert_eq!(rows.len(), 0);
    }

    #[test]
    fn serialize_quotes_all_fields() {
        let mut row = Row::new();
        row.insert("front".into(), json!("hello"));
        row.insert("back".into(), json!("world"));
        let csv = serialize_csv(&[row]).unwrap();
        assert!(csv.contains("\"front\""));
        assert!(csv.contains("\"hello\""));
    }

    #[test]
    fn serialize_empty_returns_empty_string() {
        assert_eq!(serialize_csv(&[]).unwrap(), "");
    }

    #[test]
    fn round_trip() {
        let mut row1 = Row::new();
        row1.insert("noteId".into(), json!("123"));
        row1.insert("front".into(), json!("hello"));
        row1.insert("back".into(), json!("world"));

        let mut row2 = Row::new();
        row2.insert("noteId".into(), json!("456"));
        row2.insert("front".into(), json!("foo"));
        row2.insert("back".into(), json!("bar"));

        let csv = serialize_csv(&[row1.clone(), row2.clone()]).unwrap();
        let parsed = parse_csv(&csv).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0]["noteId"], json!("123"));
        assert_eq!(parsed[0]["front"], json!("hello"));
        assert_eq!(parsed[1]["back"], json!("bar"));
    }
}

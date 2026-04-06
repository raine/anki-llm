use std::collections::HashMap;
use std::sync::LazyLock;

use jsonrepair::{Options, repair_json};
use regex::Regex;
use serde_json::Value;

use crate::data::Row;

/// Regex to extract JSON from a markdown fenced code block.
/// Relaxed whitespace matching — handles LLMs that omit newlines around fences.
static FENCED_JSON_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)```(?:json)?\s*(.*?)\s*```").unwrap());

/// Attempt to repair malformed JSON text before parsing. Returns the repaired
/// string on success, or the original text if repair fails.
fn repair(text: &str) -> String {
    repair_json(text, &Options::default()).unwrap_or_else(|_| text.to_string())
}

/// Try to parse text as a JSON object. Also tries extracting from markdown
/// fenced code blocks. Returns None if the text is not a JSON object.
pub fn try_parse_json_object(text: &str) -> Option<serde_json::Map<String, Value>> {
    let repaired = repair(text.trim());

    // Try direct parse first
    if let Some(map) = try_parse_object(&repaired) {
        return Some(map);
    }

    // Try all fenced code blocks in order — some LLMs emit a non-JSON block
    // (e.g. markdown explanation) before the JSON block.
    for caps in FENCED_JSON_RE.captures_iter(text) {
        let repaired_cap = repair(caps[1].trim());
        if let Some(map) = try_parse_object(&repaired_cap) {
            return Some(map);
        }
    }

    None
}

fn try_parse_object(text: &str) -> Option<serde_json::Map<String, Value>> {
    let value: Value = serde_json::from_str(text).ok()?;
    match value {
        Value::Object(map) => Some(map),
        _ => None,
    }
}

/// Try to parse text as a JSON array of objects. Also tries extracting from
/// markdown fenced code blocks. Returns None if the text is not a JSON array.
pub fn try_parse_json_array(text: &str) -> Option<Vec<serde_json::Map<String, Value>>> {
    let repaired = repair(text.trim());

    // Try direct parse first
    if let Some(arr) = try_parse_array(&repaired) {
        return Some(arr);
    }

    // Try fenced code blocks
    for caps in FENCED_JSON_RE.captures_iter(text) {
        let repaired_cap = repair(caps[1].trim());
        if let Some(arr) = try_parse_array(&repaired_cap) {
            return Some(arr);
        }
    }

    None
}

fn try_parse_array(text: &str) -> Option<Vec<serde_json::Map<String, Value>>> {
    let value: Value = serde_json::from_str(text).ok()?;
    match value {
        Value::Array(arr) => {
            let mut result = Vec::new();
            for item in arr {
                match item {
                    Value::Object(map) => result.push(map),
                    _ => return None, // All items must be objects
                }
            }
            Some(result)
        }
        _ => None,
    }
}

/// Merge fields from `source` into `target` using case-insensitive key matching.
///
/// - If a source key matches a target key (case-insensitive), the target value
///   is updated using the target's original key casing.
/// - If no match, the source key is added as-is.
/// - Errors if target has ambiguous keys (e.g. both "Name" and "name").
pub fn merge_fields_case_insensitive(
    target: &mut Row,
    source: &serde_json::Map<String, Value>,
) -> Result<(), String> {
    // Build lowercase -> original key mapping for target
    let mut key_map: HashMap<String, String> = HashMap::new();
    for key in target.keys() {
        let lower = key.to_lowercase();
        if key_map.contains_key(&lower) {
            return Err(format!(
                "ambiguous keys in row: '{}' and '{}' match when lowercased",
                key_map[&lower], key
            ));
        }
        key_map.insert(lower, key.clone());
    }

    for (source_key, source_value) in source {
        let lower = source_key.to_lowercase();
        if let Some(original_key) = key_map.get(&lower) {
            // Update existing field with original casing
            let original_key = original_key.clone();
            target.insert(original_key, source_value.clone());
        } else {
            // Add new field; register in key_map to catch ambiguous source keys
            // (e.g. source has both "NewKey" and "newkey").
            if key_map.contains_key(&lower) {
                return Err(format!(
                    "ambiguous keys in source: '{}' and '{}' match when lowercased",
                    key_map[&lower], source_key
                ));
            }
            key_map.insert(lower, source_key.clone());
            target.insert(source_key.clone(), source_value.clone());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_raw_object() {
        let map = try_parse_json_object(r#"{"key": "value"}"#).unwrap();
        assert_eq!(map["key"], json!("value"));
    }

    #[test]
    fn parse_fenced_block() {
        let text = "Here is the result:\n```json\n{\"key\": \"value\"}\n```\nDone.";
        let map = try_parse_json_object(text).unwrap();
        assert_eq!(map["key"], json!("value"));
    }

    #[test]
    fn parse_fenced_block_no_newlines() {
        let text = "```json{\"key\": \"value\"}```";
        let map = try_parse_json_object(text).unwrap();
        assert_eq!(map["key"], json!("value"));
    }

    #[test]
    fn parse_empty_string_returns_none() {
        assert!(try_parse_json_object("").is_none());
    }

    #[test]
    fn parse_array_returns_none() {
        assert!(try_parse_json_object("[1, 2, 3]").is_none());
    }

    #[test]
    fn parse_plain_text_returns_none() {
        assert!(try_parse_json_object("just some text").is_none());
    }

    #[test]
    fn merge_case_insensitive_update() {
        let mut target = Row::new();
        target.insert("Translation".into(), json!("old"));
        target.insert("Japanese".into(), json!("word"));

        let mut source = serde_json::Map::new();
        source.insert("translation".into(), json!("new"));

        merge_fields_case_insensitive(&mut target, &source).unwrap();
        assert_eq!(target["Translation"], json!("new"));
        assert_eq!(target.get_index(0).unwrap().0, "Translation"); // key casing preserved
    }

    #[test]
    fn merge_adds_new_field() {
        let mut target = Row::new();
        target.insert("a".into(), json!("1"));

        let mut source = serde_json::Map::new();
        source.insert("b".into(), json!("2"));

        merge_fields_case_insensitive(&mut target, &source).unwrap();
        assert_eq!(target["b"], json!("2"));
    }

    #[test]
    fn merge_ambiguous_keys_error() {
        let mut target = Row::new();
        target.insert("Name".into(), json!("a"));
        target.insert("name".into(), json!("b"));

        let source = serde_json::Map::new();
        assert!(merge_fields_case_insensitive(&mut target, &source).is_err());
    }

    #[test]
    fn parse_json_array() {
        let arr = try_parse_json_array(r#"[{"a": "1"}, {"a": "2"}]"#).unwrap();
        assert_eq!(arr.len(), 2);
    }

    #[test]
    fn parse_json_array_from_fenced() {
        let text = "Here:\n```json\n[{\"a\": \"1\"}]\n```";
        let arr = try_parse_json_array(text).unwrap();
        assert_eq!(arr.len(), 1);
    }

    #[test]
    fn parse_json_array_non_array_returns_none() {
        assert!(try_parse_json_array(r#"{"a": "1"}"#).is_none());
    }

    #[test]
    fn parse_json_array_non_objects_returns_none() {
        assert!(try_parse_json_array("[1, 2, 3]").is_none());
    }
}

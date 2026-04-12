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

/// Try to parse text as any JSON value. Runs `jsonrepair` on the input and
/// then falls back to scanning markdown fenced code blocks — some LLMs emit a
/// non-JSON block (e.g. a markdown explanation) before the JSON block.
fn try_parse_json_value(text: &str) -> Option<Value> {
    let repaired = repair(text.trim());
    if let Ok(value) = serde_json::from_str::<Value>(&repaired) {
        return Some(value);
    }

    for caps in FENCED_JSON_RE.captures_iter(text) {
        let repaired_cap = repair(caps[1].trim());
        if let Ok(value) = serde_json::from_str::<Value>(&repaired_cap) {
            return Some(value);
        }
    }

    None
}

/// Try to parse text as a JSON object. Also tries extracting from markdown
/// fenced code blocks. Returns None if the text is not a JSON object.
pub fn try_parse_json_object(text: &str) -> Option<serde_json::Map<String, Value>> {
    match try_parse_json_value(text)? {
        Value::Object(map) => Some(map),
        _ => None,
    }
}

/// Try to parse text as a JSON array of objects. Also tries extracting from
/// markdown fenced code blocks. Returns None if the text is not a JSON array.
pub fn try_parse_json_array(text: &str) -> Option<Vec<serde_json::Map<String, Value>>> {
    match try_parse_json_value(text)? {
        Value::Array(arr) => {
            let mut result = Vec::new();
            for item in arr {
                match item {
                    Value::Object(map) => result.push(map),
                    _ => return None,
                }
            }
            Some(result)
        }
        _ => None,
    }
}

/// Try to parse text as a single JSON object. Accepts either a top-level
/// object or a singleton array containing exactly one object (some LLMs
/// ignore "return a single object" instructions and wrap it in an array).
/// Rejects arrays with more than one element — callers that expect a single
/// item should not silently drop extras.
pub fn try_parse_single_json_object(text: &str) -> Option<serde_json::Map<String, Value>> {
    match try_parse_json_value(text)? {
        Value::Object(map) => Some(map),
        Value::Array(mut arr) if arr.len() == 1 => match arr.remove(0) {
            Value::Object(map) => Some(map),
            _ => None,
        },
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

    // Check source for internal ambiguity before merging (e.g. source has both
    // "NewKey" and "newkey"). Must be done upfront — once the first key is
    // inserted into key_map the second would silently match as an update.
    let mut source_lower: HashMap<String, &str> = HashMap::new();
    for source_key in source.keys() {
        let lower = source_key.to_lowercase();
        if let Some(existing) = source_lower.insert(lower, source_key) {
            return Err(format!(
                "ambiguous keys in source: '{existing}' and '{source_key}' match when lowercased",
            ));
        }
    }

    for (source_key, source_value) in source {
        let lower = source_key.to_lowercase();
        if let Some(original_key) = key_map.get(&lower) {
            // Update existing field with original casing
            let original_key = original_key.clone();
            target.insert(original_key, source_value.clone());
        } else {
            // New field — add with source casing
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
    fn merge_ambiguous_target_keys_error() {
        let mut target = Row::new();
        target.insert("Name".into(), json!("a"));
        target.insert("name".into(), json!("b"));

        let source = serde_json::Map::new();
        assert!(merge_fields_case_insensitive(&mut target, &source).is_err());
    }

    #[test]
    fn merge_ambiguous_source_keys_error() {
        let mut target = Row::new();
        target.insert("existing".into(), json!("x"));

        let mut source = serde_json::Map::new();
        source.insert("NewKey".into(), json!("a"));
        source.insert("newkey".into(), json!("b"));

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

    #[test]
    fn repair_trailing_comma_object() {
        let map = try_parse_json_object(r#"{"a": "1", "b": "2",}"#).unwrap();
        assert_eq!(map["a"], json!("1"));
        assert_eq!(map["b"], json!("2"));
    }

    #[test]
    fn repair_unquoted_keys() {
        let map = try_parse_json_object(r#"{a: "1", b: "2"}"#).unwrap();
        assert_eq!(map["a"], json!("1"));
        assert_eq!(map["b"], json!("2"));
    }

    #[test]
    fn repair_truncated_object() {
        // Missing closing brace — jsonrepair should close it.
        let map = try_parse_json_object(r#"{"a": "1", "b": "2""#).unwrap();
        assert_eq!(map["a"], json!("1"));
        assert_eq!(map["b"], json!("2"));
    }

    #[test]
    fn repair_fenced_block_with_trailing_comma() {
        let text = "Here you go:\n```json\n{\"a\": \"1\",}\n```";
        let map = try_parse_json_object(text).unwrap();
        assert_eq!(map["a"], json!("1"));
    }

    #[test]
    fn parse_single_object_top_level() {
        let map = try_parse_single_json_object(r#"{"a": "1"}"#).unwrap();
        assert_eq!(map["a"], json!("1"));
    }

    #[test]
    fn parse_single_object_from_singleton_array() {
        let map = try_parse_single_json_object(r#"[{"a": "1"}]"#).unwrap();
        assert_eq!(map["a"], json!("1"));
    }

    #[test]
    fn parse_single_object_rejects_multi_element_array() {
        assert!(try_parse_single_json_object(r#"[{"a": "1"}, {"a": "2"}]"#).is_none());
    }

    #[test]
    fn parse_single_object_from_fenced_singleton_array() {
        let text = "```json\n[{\"a\": \"1\"}]\n```";
        let map = try_parse_single_json_object(text).unwrap();
        assert_eq!(map["a"], json!("1"));
    }

    #[test]
    fn parse_single_object_rejects_non_object_array_item() {
        assert!(try_parse_single_json_object("[42]").is_none());
    }
}

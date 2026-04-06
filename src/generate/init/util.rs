use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;

static RE_SOUND: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\[sound:.*\]$").unwrap());

/// Common field name to key mappings.
fn common_mappings() -> HashMap<&'static str, &'static str> {
    let mut m = HashMap::new();
    m.insert("english", "en");
    m.insert("japanese", "jp");
    m.insert("kanji", "kanji");
    m.insert("hiragana", "hira");
    m.insert("katakana", "kata");
    m.insert("romaji", "roma");
    m.insert("meaning", "mean");
    m.insert("definition", "def");
    m.insert("example", "ex");
    m.insert("sentence", "sent");
    m.insert("front", "front");
    m.insert("back", "back");
    m.insert("reading", "read");
    m.insert("pronunciation", "pron");
    m.insert("translation", "trans");
    m.insert("notes", "notes");
    m.insert("tags", "tags");
    m
}

/// Suggest a key for a field name.
pub fn suggest_key_for_field(field_name: &str) -> String {
    let lower = field_name.to_lowercase();

    if let Some(key) = common_mappings().get(lower.as_str()) {
        return key.to_string();
    }

    // Take first 2-4 chars depending on length
    let chars: Vec<char> = lower.chars().filter(|c| c.is_alphanumeric()).collect();
    let key: String = if chars.len() <= 3 {
        chars.into_iter().collect()
    } else if chars.len() <= 5 {
        chars.into_iter().take(3).collect()
    } else {
        chars.into_iter().take(4).collect()
    };

    if key.is_empty() {
        "field".to_string()
    } else {
        key
    }
}

/// Resolve duplicate keys by appending numbers.
pub fn resolve_duplicate_keys(keys: Vec<String>) -> Vec<String> {
    let mut counts: HashMap<String, u32> = HashMap::new();
    let mut result = Vec::with_capacity(keys.len());

    for key in keys {
        let count = counts.get(&key).copied().unwrap_or(0);
        counts.insert(key.clone(), count + 1);

        let final_key = if count == 0 {
            key
        } else {
            format!("{key}{}", count + 1)
        };
        result.push(final_key);
    }

    result
}

/// Check if a field value looks auto-generated (e.g., sound files).
pub fn is_auto_generated_field(value: &str) -> bool {
    RE_SOUND.is_match(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggest_key_common_fields() {
        assert_eq!(suggest_key_for_field("english"), "en");
        assert_eq!(suggest_key_for_field("japanese"), "jp");
        assert_eq!(suggest_key_for_field("kanji"), "kanji");
        assert_eq!(suggest_key_for_field("front"), "front");
        assert_eq!(suggest_key_for_field("back"), "back");
    }

    #[test]
    fn suggest_key_unknown_fields() {
        // Short field
        assert_eq!(suggest_key_for_field("ab"), "ab");
        // Medium field
        assert_eq!(suggest_key_for_field("abcd"), "abc");
        // Longer field
        assert_eq!(suggest_key_for_field("abcdefg"), "abcd");
        // Empty after filtering
        assert_eq!(suggest_key_for_field("123"), "123");
    }

    #[test]
    fn resolve_duplicate_keys_no_duplicates() {
        let keys = vec!["front".to_string(), "back".to_string()];
        let resolved = resolve_duplicate_keys(keys);
        assert_eq!(resolved, vec!["front", "back"]);
    }

    #[test]
    fn resolve_duplicate_keys_with_duplicates() {
        let keys = vec![
            "front".to_string(),
            "back".to_string(),
            "front".to_string(),
            "front".to_string(),
        ];
        let resolved = resolve_duplicate_keys(keys);
        assert_eq!(resolved, vec!["front", "back", "front2", "front3"]);
    }

    #[test]
    fn is_auto_generated_field_sound() {
        assert!(is_auto_generated_field("[sound:test.mp3]"));
        assert!(is_auto_generated_field("[sound:foobar.wav]"));
        assert!(!is_auto_generated_field("regular text"));
        assert!(!is_auto_generated_field("[note:something]"));
    }
}

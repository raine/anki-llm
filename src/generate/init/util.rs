use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;

/// Matches fields that consist entirely of Anki media tokens (sound, image) and whitespace.
static RE_MEDIA_ONLY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:\s*\[sound:[^\]]*\]\s*|\s*<img\b[^>]*/?\s*>\s*)+$").unwrap());

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

/// Resolve duplicate keys by appending numbers, guaranteed unique.
pub fn resolve_duplicate_keys(keys: Vec<String>) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut result = Vec::with_capacity(keys.len());

    for key in keys {
        let mut final_key = key.clone();
        let mut counter = 2u32;
        while seen.contains(&final_key) {
            final_key = format!("{key}{counter}");
            counter += 1;
        }
        seen.insert(final_key.clone());
        result.push(final_key);
    }

    result
}

/// Check if a field value looks auto-generated (sound files, images).
pub fn is_auto_generated_field(value: &str) -> bool {
    !value.trim().is_empty() && RE_MEDIA_ONLY.is_match(value)
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
    fn resolve_duplicate_keys_no_collision_with_preexisting_numbered() {
        // ["front", "front2", "front"] must not produce ["front", "front2", "front2"]
        let keys = vec![
            "front".to_string(),
            "front2".to_string(),
            "front".to_string(),
        ];
        let resolved = resolve_duplicate_keys(keys);
        let unique: std::collections::HashSet<_> = resolved.iter().collect();
        assert_eq!(unique.len(), 3, "all keys must be unique: {resolved:?}");
    }

    #[test]
    fn is_auto_generated_field_sound() {
        assert!(is_auto_generated_field("[sound:test.mp3]"));
        assert!(is_auto_generated_field("[sound:foobar.wav]"));
        assert!(!is_auto_generated_field("regular text"));
        assert!(!is_auto_generated_field("[note:something]"));
        assert!(!is_auto_generated_field(""));
    }

    #[test]
    fn is_auto_generated_field_image() {
        assert!(is_auto_generated_field("<img src=\"foo.jpg\">"));
        assert!(is_auto_generated_field("<img src=\"foo.jpg\"/>"));
        assert!(is_auto_generated_field("[sound:a.mp3][sound:b.mp3]"));
        assert!(is_auto_generated_field("  [sound:a.mp3]  "));
    }
}

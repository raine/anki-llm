use std::sync::LazyLock;

use regex::Regex;

static RE_INVALID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-z0-9\s-]").unwrap());
static RE_COLLAPSE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\s-]+").unwrap());

/// Convert a deck name to a safe filename slug.
/// Uses the last segment of `::` separated deck names.
/// Returns "unnamed" if the result would be empty.
pub fn slugify_deck_name(name: &str) -> String {
    let last_part = name.split("::").last().unwrap_or(name);
    let s = last_part.to_lowercase();
    let s = s.trim();
    let s = RE_INVALID.replace_all(s, "");
    let s = RE_COLLAPSE.replace_all(&s, "-");
    let result = s.trim_matches('-').to_string();
    if result.is_empty() {
        "unnamed".to_string()
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_name() {
        assert_eq!(slugify_deck_name("My Deck"), "my-deck");
    }

    #[test]
    fn sub_deck() {
        assert_eq!(slugify_deck_name("Japanese::Vocabulary"), "vocabulary");
    }

    #[test]
    fn special_chars() {
        assert_eq!(slugify_deck_name("Café & Résumé"), "caf-rsum");
    }

    #[test]
    fn already_clean() {
        assert_eq!(slugify_deck_name("simple"), "simple");
    }

    #[test]
    fn all_special_chars_fallback() {
        assert_eq!(slugify_deck_name(":::"), "unnamed");
    }
}

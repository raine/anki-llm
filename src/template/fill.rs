use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;

use super::error::TemplateError;
use crate::data::Row;

// Field names can be Unicode (Anki allows e.g. Japanese names). Match any
// non-empty run of characters that aren't `{` `}` or whitespace, which
// covers ASCII identifiers as well as `日本語`-style names.
pub(crate) static PLACEHOLDER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{([^\s{}]+)\}").unwrap());

/// Fill a template string by replacing `{key}` placeholders with values from `row`.
///
/// Matching is case-insensitive. Throws on ambiguous keys (e.g. row has both
/// "Name" and "name") or missing placeholders.
pub fn fill_template(template: &str, row: &Row) -> Result<String, TemplateError> {
    // Build case-insensitive lookup: lowercase key -> (original key, value string)
    let mut lookup: HashMap<String, (&str, String)> = HashMap::new();
    for (key, value) in row {
        let lower = key.to_lowercase();
        if let Some((existing_key, _)) = lookup.get(&lower) {
            return Err(TemplateError::AmbiguousKeys(
                existing_key.to_string(),
                key.clone(),
            ));
        }
        let str_val = match value {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Null => String::new(),
            other => other.to_string(),
        };
        lookup.insert(lower, (key.as_str(), str_val));
    }

    // Collect all required placeholder keys and check for missing ones
    let mut missing = Vec::new();
    for cap in PLACEHOLDER_RE.captures_iter(template) {
        let key = &cap[1];
        let lower = key.to_lowercase();
        if !lookup.contains_key(&lower) {
            missing.push(format!("{{{key}}}"));
        }
    }
    // Deduplicate
    missing.sort();
    missing.dedup();
    if !missing.is_empty() {
        return Err(TemplateError::MissingPlaceholders(missing.join(", ")));
    }

    // Replace all placeholders in one pass
    let result = PLACEHOLDER_RE.replace_all(template, |caps: &regex::Captures| {
        let key = &caps[1];
        let lower = key.to_lowercase();
        lookup
            .get(&lower)
            .map(|(_, v)| v.as_str())
            .unwrap_or("")
            .to_string()
    });

    Ok(result.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn row(pairs: &[(&str, &str)]) -> Row {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
            .collect()
    }

    #[test]
    fn basic_fill() {
        let r = row(&[("term", "hello")]);
        assert_eq!(fill_template("Say {term}", &r).unwrap(), "Say hello");
    }

    #[test]
    fn case_insensitive() {
        let r = row(&[("term", "hello")]);
        assert_eq!(fill_template("{Term}", &r).unwrap(), "hello");
    }

    #[test]
    fn ambiguous_keys() {
        let mut r = Row::new();
        r.insert("Name".into(), Value::String("a".into()));
        r.insert("name".into(), Value::String("b".into()));
        assert!(fill_template("{name}", &r).is_err());
    }

    #[test]
    fn missing_placeholder() {
        let r = row(&[("term", "hello")]);
        let err = fill_template("{term} {missing}", &r).unwrap_err();
        assert!(err.to_string().contains("{missing}"));
    }

    #[test]
    fn unicode_field_name() {
        let mut r = Row::new();
        r.insert("日本語".into(), Value::String("こんにちは".into()));
        assert_eq!(fill_template("{日本語}", &r).unwrap(), "こんにちは");
    }

    #[test]
    fn null_value() {
        let mut r = Row::new();
        r.insert("x".into(), Value::Null);
        assert_eq!(fill_template("{x}", &r).unwrap(), "");
    }

    #[test]
    fn number_value() {
        let mut r = Row::new();
        r.insert("n".into(), Value::Number(42.into()));
        assert_eq!(fill_template("{n}", &r).unwrap(), "42");
    }

    #[test]
    fn no_placeholders() {
        let r = row(&[("term", "hello")]);
        assert_eq!(
            fill_template("no placeholders here", &r).unwrap(),
            "no placeholders here"
        );
    }

    #[test]
    fn multiple_same_placeholder() {
        let r = row(&[("x", "val")]);
        assert_eq!(fill_template("{x} and {x}", &r).unwrap(), "val and val");
    }
}

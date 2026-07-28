use serde::{Deserialize, Serialize};

use super::error::TemplateError;

/// Frontmatter schema for `process-deck` / `process-file` prompts.
///
/// Distinct from generate's `Frontmatter`: process prompts have no
/// `field_map` or tts configuration. Templates reference raw field names.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessPrompt {
    /// Human-readable title (not yet surfaced, reserved for a future
    /// process-prompt picker).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Short description (same provenance as `title`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub output: ProcessOutputBlock,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessOutputBlock {
    /// Anki field name that receives an unstructured text response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    /// Field names received together in a structured JSON response.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<String>,
    /// When true, only the content inside the last `<result>...</result>`
    /// pair in a single-field response is written to the field. If no tags are
    /// present, the row fails.
    #[serde(default)]
    pub require_result_tag: bool,
}

impl ProcessOutputBlock {
    pub fn field_names(&self) -> Vec<String> {
        self.field
            .iter()
            .cloned()
            .chain(self.fields.iter().cloned())
            .collect()
    }

    pub fn is_structured(&self) -> bool {
        !self.fields.is_empty()
    }
}

#[derive(Debug)]
pub struct ParsedProcessPrompt {
    pub frontmatter: ProcessPrompt,
    pub body: String,
}

/// Parse a `process-*` prompt file. Frontmatter is required.
pub fn parse(content: &str) -> Result<ParsedProcessPrompt, TemplateError> {
    let re = regex::Regex::new(r"(?s)^---\s*\n(.*?)\n---\s*\n(.*)$").unwrap();
    let caps = re.captures(content).ok_or_else(|| {
        TemplateError::InvalidFrontmatter(
            "process-* prompts require a YAML frontmatter block enclosed by --- markers. \
             Declare `output.field` or `output.fields` there."
                .into(),
        )
    })?;

    let yaml_text = &caps[1];
    let body = caps[2].trim().to_string();

    let frontmatter: ProcessPrompt = serde_yaml::from_str(yaml_text).map_err(|e| {
        TemplateError::InvalidFrontmatter(format!("Failed to parse frontmatter: {e}"))
    })?;

    let has_field = frontmatter.output.field.is_some();
    let has_fields = !frontmatter.output.fields.is_empty();
    if has_field == has_fields {
        return Err(TemplateError::InvalidFrontmatter(
            "declare exactly one of output.field or output.fields".into(),
        ));
    }

    if frontmatter
        .output
        .field
        .as_ref()
        .is_some_and(|field| field.trim().is_empty())
    {
        return Err(TemplateError::InvalidFrontmatter(
            "output.field must be non-empty".into(),
        ));
    }

    if has_fields && frontmatter.output.fields.len() < 2 {
        return Err(TemplateError::InvalidFrontmatter(
            "output.fields must contain at least two fields; use output.field for one field".into(),
        ));
    }

    if frontmatter
        .output
        .fields
        .iter()
        .any(|field| field.trim().is_empty())
    {
        return Err(TemplateError::InvalidFrontmatter(
            "output.fields entries must be non-empty".into(),
        ));
    }

    let mut unique_fields = std::collections::HashSet::new();
    if frontmatter
        .output
        .field_names()
        .iter()
        .any(|field| !unique_fields.insert(field))
    {
        return Err(TemplateError::InvalidFrontmatter(
            "output fields must be unique".into(),
        ));
    }

    if has_fields && frontmatter.output.require_result_tag {
        return Err(TemplateError::InvalidFrontmatter(
            "output.require_result_tag is only supported with output.field".into(),
        ));
    }

    if body.is_empty() {
        return Err(TemplateError::InvalidPrompt("prompt body is empty".into()));
    }

    Ok(ParsedProcessPrompt { frontmatter, body })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_prompt() {
        let content = "---\n\
output:\n  \
field: Hint\n\
---\n\n\
body here";
        let parsed = parse(content).unwrap();
        assert_eq!(parsed.frontmatter.output.field.as_deref(), Some("Hint"));
        assert!(!parsed.frontmatter.output.require_result_tag);
        assert_eq!(parsed.body, "body here");
    }

    #[test]
    fn parses_full_prompt() {
        let content = "---\n\
title: Hint generator\n\
description: Writes subtle hints\n\
output:\n  \
field: Hint\n  \
require_result_tag: true\n\
---\n\n\
English: {English}";
        let parsed = parse(content).unwrap();
        assert_eq!(parsed.frontmatter.title.as_deref(), Some("Hint generator"));
        assert!(parsed.frontmatter.output.require_result_tag);
        assert_eq!(parsed.body, "English: {English}");
    }

    #[test]
    fn parses_multi_field_prompt() {
        let content = "---\n\
output:\n  \
fields:\n    - Reading\n    - Explanation\n    - KanjiBreakdown\n\
---\n\n\
Japanese: {Kanji}";
        let parsed = parse(content).unwrap();
        assert_eq!(
            parsed.frontmatter.output.field_names(),
            ["Reading", "Explanation", "KanjiBreakdown"]
        );
        assert!(parsed.frontmatter.output.is_structured());
    }

    #[test]
    fn rejects_field_and_fields_together() {
        let content = "---\n\
output:\n  \
field: Reading\n  \
fields:\n    - Explanation\n\
---\n\n\
body";
        assert!(parse(content).is_err());
    }

    #[test]
    fn rejects_single_entry_fields_list() {
        let content = "---\n\
output:\n  \
fields:\n    - Reading\n\
---\n\n\
body";
        assert!(parse(content).is_err());
    }

    #[test]
    fn rejects_duplicate_multi_fields() {
        let content = "---\n\
output:\n  \
fields:\n    - Reading\n    - Reading\n\
---\n\n\
body";
        assert!(parse(content).is_err());
    }

    #[test]
    fn rejects_result_tags_for_multi_field_prompt() {
        let content = "---\n\
output:\n  \
fields:\n    - Reading\n    - Explanation\n  \
require_result_tag: true\n\
---\n\n\
body";
        assert!(parse(content).is_err());
    }

    #[test]
    fn rejects_missing_frontmatter() {
        let err = parse("just raw text, no frontmatter").unwrap_err();
        assert!(err.to_string().contains("frontmatter"));
    }

    #[test]
    fn rejects_unknown_fields() {
        let content = "---\n\
output:\n  \
field: Hint\n\
extra: nope\n\
---\n\n\
body";
        assert!(parse(content).is_err());
    }

    #[test]
    fn rejects_unknown_output_fields() {
        let content = "---\n\
output:\n  \
field: Hint\n  \
format: text\n\
---\n\n\
body";
        let err = parse(content).unwrap_err();
        assert!(err.to_string().contains("format"));
    }

    #[test]
    fn rejects_empty_field() {
        let content = "---\n\
output:\n  \
field: ''\n\
---\n\n\
body";
        assert!(parse(content).is_err());
    }

    #[test]
    fn rejects_missing_output() {
        let content = "---\n\
title: something\n\
---\n\n\
body";
        assert!(parse(content).is_err());
    }

    #[test]
    fn rejects_empty_body() {
        let content = "---\n\
output:\n  \
field: Hint\n\
---\n";
        let err = parse(content).unwrap_err();
        assert!(err.to_string().contains("body"));
    }
}

use serde::{Deserialize, Serialize};

use super::error::TemplateError;

/// Frontmatter schema for `process-deck` / `process-file` prompts.
///
/// Distinct from generate's `Frontmatter` — process prompts have no
/// `field_map`, no tts, and only produce text output written to a
/// single Anki field. Templates reference raw Anki field names.
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
    /// Anki field name that receives the LLM response text.
    pub field: String,
    /// When true, only the content inside the last `<result>...</result>`
    /// pair in the response is written to the field. If no tags are
    /// present, the row fails.
    #[serde(default)]
    pub require_result_tag: bool,
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
             Declare `output.field` (and optionally `output.require_result_tag`) there."
                .into(),
        )
    })?;

    let yaml_text = &caps[1];
    let body = caps[2].trim().to_string();

    let frontmatter: ProcessPrompt = serde_yaml::from_str(yaml_text).map_err(|e| {
        TemplateError::InvalidFrontmatter(format!("Failed to parse frontmatter: {e}"))
    })?;

    if frontmatter.output.field.trim().is_empty() {
        return Err(TemplateError::InvalidFrontmatter(
            "output.field is required and must be non-empty".into(),
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
        assert_eq!(parsed.frontmatter.output.field, "Hint");
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

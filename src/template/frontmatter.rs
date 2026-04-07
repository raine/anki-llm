use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use super::error::TemplateError;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Frontmatter {
    pub deck: String,
    pub note_type: String,
    pub field_map: IndexMap<String, String>,
    #[serde(default)]
    pub quality_check: Option<QualityCheckConfig>,
    #[serde(default)]
    pub post_process: Vec<PostProcessConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityCheckConfig {
    pub field: String,
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostProcessConfig {
    /// Target field to update. If set, accepts plain text or `{"value": "..."}`.
    /// If omitted, expects a JSON object whose keys are merged into the card.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

pub struct ParsedPromptFile {
    pub frontmatter: Frontmatter,
    pub body: String,
}

/// Parse a prompt file with YAML frontmatter between `---` markers.
pub fn parse_prompt_file(content: &str) -> Result<ParsedPromptFile, TemplateError> {
    let re = regex::Regex::new(r"(?s)^---\s*\n(.*?)\n---\s*\n(.*)$").unwrap();
    let caps = re.captures(content).ok_or_else(|| {
        TemplateError::InvalidFrontmatter(
            "Expected YAML frontmatter enclosed by --- markers".into(),
        )
    })?;

    let yaml_text = &caps[1];
    let body = caps[2].trim().to_string();

    let frontmatter: Frontmatter = serde_yaml::from_str(yaml_text).map_err(|e| {
        TemplateError::InvalidFrontmatter(format!("Failed to parse frontmatter: {e}"))
    })?;

    // Validate required fields
    if frontmatter.deck.is_empty() {
        return Err(TemplateError::InvalidFrontmatter("deck is required".into()));
    }
    if frontmatter.note_type.is_empty() {
        return Err(TemplateError::InvalidFrontmatter(
            "note_type is required".into(),
        ));
    }
    if frontmatter.field_map.is_empty() {
        return Err(TemplateError::InvalidFrontmatter(
            "field_map must have at least one entry".into(),
        ));
    }

    if let Some(ref qc) = frontmatter.quality_check
        && (qc.field.is_empty() || qc.prompt.is_empty())
    {
        return Err(TemplateError::InvalidFrontmatter(
            "quality_check requires both field and prompt".into(),
        ));
    }

    for task in &frontmatter.post_process {
        if task.prompt.is_empty() {
            return Err(TemplateError::InvalidFrontmatter(
                "each post_process entry requires a prompt".into(),
            ));
        }
        if let Some(ref target) = task.target {
            if target.is_empty() {
                return Err(TemplateError::InvalidFrontmatter(
                    "post_process target must not be empty".into(),
                ));
            }
            if !frontmatter.field_map.contains_key(target) {
                return Err(TemplateError::InvalidFrontmatter(format!(
                    "post_process target '{target}' must be a key in field_map",
                )));
            }
        }
    }

    Ok(ParsedPromptFile { frontmatter, body })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_frontmatter() {
        let content = "---
deck: Test
note_type: Basic
field_map:
  front: Front
  back: Back
---

Hello {term}";
        let parsed = parse_prompt_file(content).unwrap();
        assert_eq!(parsed.frontmatter.deck, "Test");
        assert_eq!(parsed.frontmatter.note_type, "Basic");
        assert_eq!(parsed.frontmatter.field_map.len(), 2);
        assert_eq!(parsed.body, "Hello {term}");
    }

    #[test]
    fn parse_with_quality_check() {
        let content = "---
deck: Test
note_type: Basic
field_map:
  front: Front
quality_check:
  field: front
  prompt: Check {text}
---

body";
        let parsed = parse_prompt_file(content).unwrap();
        assert!(parsed.frontmatter.quality_check.is_some());
        let qc = parsed.frontmatter.quality_check.unwrap();
        assert_eq!(qc.field, "front");
    }

    #[test]
    fn missing_frontmatter_markers() {
        let content = "no frontmatter here";
        assert!(parse_prompt_file(content).is_err());
    }

    #[test]
    fn parse_with_post_process_target() {
        let content = "---
deck: Test
note_type: Basic
field_map:
  front: Front
  read: Reading
post_process:
  - target: read
    prompt: Fix {front}
---

body";
        let parsed = parse_prompt_file(content).unwrap();
        assert_eq!(parsed.frontmatter.post_process.len(), 1);
        let task = &parsed.frontmatter.post_process[0];
        assert_eq!(task.target.as_deref(), Some("read"));
        assert_eq!(task.prompt, "Fix {front}");
    }

    #[test]
    fn parse_with_post_process_no_target() {
        let content = "---
deck: Test
note_type: Basic
field_map:
  front: Front
  read: Reading
post_process:
  - prompt: Return JSON with read and front
---

body";
        let parsed = parse_prompt_file(content).unwrap();
        assert_eq!(parsed.frontmatter.post_process.len(), 1);
        assert!(parsed.frontmatter.post_process[0].target.is_none());
    }

    #[test]
    fn post_process_unknown_target() {
        let content = "---
deck: Test
note_type: Basic
field_map:
  front: Front
post_process:
  - target: nonexistent
    prompt: Fix it
---

body";
        assert!(parse_prompt_file(content).is_err());
    }

    #[test]
    fn post_process_empty_prompt() {
        let content = "---
deck: Test
note_type: Basic
field_map:
  front: Front
post_process:
  - target: front
    prompt: ''
---

body";
        assert!(parse_prompt_file(content).is_err());
    }

    #[test]
    fn empty_field_map() {
        let content = "---
deck: Test
note_type: Basic
field_map: {}
---

body";
        assert!(parse_prompt_file(content).is_err());
    }
}

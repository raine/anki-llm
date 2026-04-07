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
    pub field_tasks: Vec<FieldTaskConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityCheckConfig {
    pub field: String,
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldTaskConfig {
    pub field: String,
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

    for task in &frontmatter.field_tasks {
        if task.field.is_empty() || task.prompt.is_empty() {
            return Err(TemplateError::InvalidFrontmatter(
                "each fieldTask requires both field and prompt".into(),
            ));
        }
        if !frontmatter.field_map.contains_key(&task.field) {
            return Err(TemplateError::InvalidFrontmatter(format!(
                "fieldTask target '{}' must be a key in fieldMap",
                task.field
            )));
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
    fn parse_with_field_tasks() {
        let content = "---
deck: Test
noteType: Basic
fieldMap:
  front: Front
  read: Reading
fieldTasks:
  - field: read
    prompt: Fix {front}
---

body";
        let parsed = parse_prompt_file(content).unwrap();
        assert_eq!(parsed.frontmatter.field_tasks.len(), 1);
        let task = &parsed.frontmatter.field_tasks[0];
        assert_eq!(task.field, "read");
        assert_eq!(task.prompt, "Fix {front}");
    }

    #[test]
    fn field_task_unknown_field() {
        let content = "---
deck: Test
noteType: Basic
fieldMap:
  front: Front
fieldTasks:
  - field: nonexistent
    prompt: Fix it
---

body";
        assert!(parse_prompt_file(content).is_err());
    }

    #[test]
    fn field_task_empty_prompt() {
        let content = "---
deck: Test
noteType: Basic
fieldMap:
  front: Front
fieldTasks:
  - field: front
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

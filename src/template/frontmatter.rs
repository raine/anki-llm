use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use super::error::TemplateError;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Frontmatter {
    /// Human-readable title for prompt picker (falls back to filename).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Short description shown in prompt picker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub deck: String,
    pub note_type: String,
    pub field_map: IndexMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub processing: Option<ProcessingConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ProcessorKind {
    Transform,
    Check,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessorStep {
    #[serde(rename = "type")]
    pub kind: ProcessorKind,
    /// Single field to write (shorthand for writes: [target]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Multiple fields to write. Mutually exclusive with target.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub writes: Vec<String>,
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl ProcessorStep {
    /// Effective write fields. target is shorthand for writes: [target].
    pub fn write_fields(&self) -> Vec<&str> {
        if let Some(ref t) = self.target {
            vec![t.as_str()]
        } else {
            self.writes.iter().map(|s| s.as_str()).collect()
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProcessingConfig {
    #[serde(default)]
    pub pre_select: Vec<ProcessorStep>,
    #[serde(default)]
    pub post_select: Vec<ProcessorStep>,
}

#[derive(Debug)]
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

    // Validate processing config
    if let Some(ref processing) = frontmatter.processing {
        for step in processing
            .pre_select
            .iter()
            .chain(processing.post_select.iter())
        {
            if step.prompt.is_empty() {
                return Err(TemplateError::InvalidFrontmatter(
                    "each processing step requires a prompt".into(),
                ));
            }
            match step.kind {
                ProcessorKind::Transform => {
                    let fields = step.write_fields();
                    if fields.is_empty() {
                        return Err(TemplateError::InvalidFrontmatter(
                            "transform steps must have target or writes".into(),
                        ));
                    }
                    for f in &fields {
                        if !frontmatter.field_map.contains_key(*f) {
                            return Err(TemplateError::InvalidFrontmatter(format!(
                                "processing target/writes field '{f}' must be a key in field_map",
                            )));
                        }
                    }
                }
                ProcessorKind::Check => {
                    if step.target.is_some() || !step.writes.is_empty() {
                        return Err(TemplateError::InvalidFrontmatter(
                            "check steps must not have target or writes".into(),
                        ));
                    }
                }
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
    fn missing_frontmatter_markers() {
        let content = "no frontmatter here";
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

    #[test]
    fn parse_processing_config() {
        let content = "---
deck: Test
note_type: Basic
field_map:
  front: Front
  back: Back
  reading: Reading
processing:
  pre_select:
    - type: transform
      target: reading
      prompt: Add furigana to {front}
    - type: check
      prompt: Is this card accurate?
  post_select:
    - type: transform
      writes: [back]
      prompt: Improve {back}
---

body";
        let parsed = parse_prompt_file(content).unwrap();
        let processing = parsed.frontmatter.processing.unwrap();
        assert_eq!(processing.pre_select.len(), 2);
        assert_eq!(processing.post_select.len(), 1);
        assert_eq!(processing.pre_select[0].kind, ProcessorKind::Transform);
        assert_eq!(processing.pre_select[0].write_fields(), vec!["reading"]);
        assert_eq!(processing.pre_select[1].kind, ProcessorKind::Check);
        assert_eq!(processing.post_select[0].write_fields(), vec!["back"]);
    }

    #[test]
    fn processing_transform_requires_target_or_writes() {
        let content = "---
deck: Test
note_type: Basic
field_map:
  front: Front
processing:
  pre_select:
    - type: transform
      prompt: Do something
---

body";
        assert!(parse_prompt_file(content).is_err());
    }

    #[test]
    fn processing_check_rejects_target() {
        let content = "---
deck: Test
note_type: Basic
field_map:
  front: Front
processing:
  pre_select:
    - type: check
      target: front
      prompt: Check it
---

body";
        assert!(parse_prompt_file(content).is_err());
    }

    #[test]
    fn processing_field_must_be_in_field_map() {
        let content = "---
deck: Test
note_type: Basic
field_map:
  front: Front
processing:
  pre_select:
    - type: transform
      target: nonexistent
      prompt: Fix it
---

body";
        assert!(parse_prompt_file(content).is_err());
    }

    #[test]
    fn legacy_post_process_rejected() {
        let content = "---
deck: Test
note_type: Basic
field_map:
  front: Front
post_process:
  - target: front
    prompt: Fix it
---

body";
        let err = parse_prompt_file(content).unwrap_err();
        assert!(
            err.to_string().contains("unknown field"),
            "expected unknown field error, got: {err}"
        );
    }

    #[test]
    fn legacy_quality_check_rejected() {
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
        let err = parse_prompt_file(content).unwrap_err();
        assert!(
            err.to_string().contains("unknown field"),
            "expected unknown field error, got: {err}"
        );
    }
}

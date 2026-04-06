use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use super::error::TemplateError;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Frontmatter {
    pub deck: String,
    pub note_type: String,
    pub field_map: IndexMap<String, String>,
    #[serde(default)]
    pub quality_check: Option<QualityCheckConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityCheckConfig {
    pub field: String,
    pub prompt: String,
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
            "noteType is required".into(),
        ));
    }
    if frontmatter.field_map.is_empty() {
        return Err(TemplateError::InvalidFrontmatter(
            "fieldMap must have at least one entry".into(),
        ));
    }

    if let Some(ref qc) = frontmatter.quality_check
        && (qc.field.is_empty() || qc.prompt.is_empty())
    {
        return Err(TemplateError::InvalidFrontmatter(
            "qualityCheck requires both field and prompt".into(),
        ));
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
noteType: Basic
fieldMap:
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
noteType: Basic
fieldMap:
  front: Front
qualityCheck:
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
    fn empty_field_map() {
        let content = "---
deck: Test
noteType: Basic
fieldMap: {}
---

body";
        assert!(parse_prompt_file(content).is_err());
    }
}

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
    /// Optional TTS audio generation settings. When present, `anki-llm tts
    /// --prompt <file>` reads this block; `generate` validates it but
    /// does not yet execute it (see history/2026-04-13-tts-generate-design.md).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tts: Option<TtsSpec>,
}

/// Deck-level TTS configuration. Voice, target field, and source-text
/// strategy for a deck's audio are inherent to the deck's design and live
/// alongside `field_map` and `processing`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TtsSpec {
    /// Anki field name that receives the `[sound:...]` tag. Note: this is
    /// an Anki field name, not a `field_map` key — audio is not an
    /// LLM-generated output.
    pub target: String,
    /// Where the spoken text comes from.
    pub source: TtsSource,
    /// Provider voice identifier (e.g. "alloy", "nova").
    pub voice: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,
}

/// Exactly one of `field` / `template` must be set. Both reference
/// `field_map` keys (LLM-facing names), not Anki field names.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TtsSource {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
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

    if let Some(ref tts) = frontmatter.tts {
        if tts.target.trim().is_empty() {
            return Err(TemplateError::InvalidFrontmatter(
                "tts.target is required".into(),
            ));
        }
        if tts.voice.trim().is_empty() {
            return Err(TemplateError::InvalidFrontmatter(
                "tts.voice is required".into(),
            ));
        }
        if let Some(speed) = tts.speed
            && !(speed > 0.0)
        {
            return Err(TemplateError::InvalidFrontmatter(
                "tts.speed must be > 0".into(),
            ));
        }
        match (&tts.source.field, &tts.source.template) {
            (Some(_), Some(_)) | (None, None) => {
                return Err(TemplateError::InvalidFrontmatter(
                    "tts.source must set exactly one of `field` or `template`".into(),
                ));
            }
            (Some(field), None) => {
                if !frontmatter.field_map.contains_key(field) {
                    return Err(TemplateError::InvalidFrontmatter(format!(
                        "tts.source.field '{field}' is not a key in field_map",
                    )));
                }
            }
            (None, Some(template)) => {
                for cap in crate::template::fill::PLACEHOLDER_RE.captures_iter(template) {
                    let key = &cap[1];
                    if !frontmatter
                        .field_map
                        .keys()
                        .any(|k| k.eq_ignore_ascii_case(key))
                    {
                        return Err(TemplateError::InvalidFrontmatter(format!(
                            "tts.source.template references '{{{key}}}' which is not a key in field_map",
                        )));
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

    // ---- TTS spec tests -------------------------------------------------

    #[test]
    fn parse_tts_block_with_field_source() {
        let content = "---
deck: Test
note_type: Basic
field_map:
  front: Front
  back: Back
tts:
  target: Audio
  source:
    field: front
  voice: alloy
---

body";
        let parsed = parse_prompt_file(content).unwrap();
        let tts = parsed.frontmatter.tts.unwrap();
        assert_eq!(tts.target, "Audio");
        assert_eq!(tts.voice, "alloy");
        assert_eq!(tts.source.field.as_deref(), Some("front"));
        assert!(tts.source.template.is_none());
    }

    #[test]
    fn parse_tts_block_with_template_source() {
        let content = "---
deck: Test
note_type: Basic
field_map:
  front: Front
  back: Back
tts:
  target: Audio
  source:
    template: '{front} - {back}'
  voice: nova
  provider: openai
  model: gpt-4o-mini-tts
  format: mp3
  speed: 1.25
---

body";
        let parsed = parse_prompt_file(content).unwrap();
        let tts = parsed.frontmatter.tts.unwrap();
        assert_eq!(
            tts.source.template.as_deref(),
            Some("{front} - {back}")
        );
        assert_eq!(tts.model.as_deref(), Some("gpt-4o-mini-tts"));
        assert_eq!(tts.speed, Some(1.25));
    }

    #[test]
    fn tts_without_voice_rejected() {
        let content = "---
deck: Test
note_type: Basic
field_map:
  front: Front
tts:
  target: Audio
  source:
    field: front
---

body";
        assert!(parse_prompt_file(content).is_err());
    }

    #[test]
    fn tts_without_target_rejected() {
        let content = "---
deck: Test
note_type: Basic
field_map:
  front: Front
tts:
  target: ''
  source:
    field: front
  voice: alloy
---

body";
        let err = parse_prompt_file(content).unwrap_err();
        assert!(err.to_string().contains("tts.target"));
    }

    #[test]
    fn tts_source_both_field_and_template_rejected() {
        let content = "---
deck: Test
note_type: Basic
field_map:
  front: Front
tts:
  target: Audio
  source:
    field: front
    template: '{front}'
  voice: alloy
---

body";
        let err = parse_prompt_file(content).unwrap_err();
        assert!(err.to_string().contains("exactly one"));
    }

    #[test]
    fn tts_source_neither_rejected() {
        let content = "---
deck: Test
note_type: Basic
field_map:
  front: Front
tts:
  target: Audio
  source: {}
  voice: alloy
---

body";
        let err = parse_prompt_file(content).unwrap_err();
        assert!(err.to_string().contains("exactly one"));
    }

    #[test]
    fn tts_source_field_must_be_in_field_map() {
        let content = "---
deck: Test
note_type: Basic
field_map:
  front: Front
tts:
  target: Audio
  source:
    field: missing
  voice: alloy
---

body";
        let err = parse_prompt_file(content).unwrap_err();
        assert!(err.to_string().contains("not a key in field_map"));
    }

    #[test]
    fn tts_template_placeholder_must_be_in_field_map() {
        let content = "---
deck: Test
note_type: Basic
field_map:
  front: Front
tts:
  target: Audio
  source:
    template: '{missing}'
  voice: alloy
---

body";
        let err = parse_prompt_file(content).unwrap_err();
        assert!(err.to_string().contains("{missing}"));
    }

    #[test]
    fn tts_speed_must_be_positive() {
        let content = "---
deck: Test
note_type: Basic
field_map:
  front: Front
tts:
  target: Audio
  source:
    field: front
  voice: alloy
  speed: -1.0
---

body";
        let err = parse_prompt_file(content).unwrap_err();
        assert!(err.to_string().contains("speed"));
    }

    #[test]
    fn tts_absent_is_fine() {
        let content = "---
deck: Test
note_type: Basic
field_map:
  front: Front
---

body";
        let parsed = parse_prompt_file(content).unwrap();
        assert!(parsed.frontmatter.tts.is_none());
    }
}

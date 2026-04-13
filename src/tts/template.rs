use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::data::Row;
use crate::template::fill_template;

/// How the raw-text input for a single note is derived.
///
/// TTS commands accept either a prompt template (file on disk or inline
/// string from a YAML `tts.source.template`) expanded per row with
/// `{field}` placeholders, or a bare field reference for the common
/// "just speak this field" case.
#[derive(Debug, Clone)]
pub enum TemplateSource {
    File { path: PathBuf, contents: String },
    Inline { label: String, contents: String },
    Field(String),
}

impl TemplateSource {
    pub fn load_file(path: PathBuf) -> Result<Self> {
        let contents = fs::read_to_string(&path)
            .with_context(|| format!("failed to read template file: {}", path.display()))?;
        Ok(Self::File { path, contents })
    }

    pub fn inline(label: String, contents: String) -> Self {
        Self::Inline { label, contents }
    }

    pub fn field(name: String) -> Self {
        Self::Field(name)
    }

    pub fn display_label(&self) -> String {
        match self {
            Self::File { path, .. } => path.display().to_string(),
            Self::Inline { label, .. } => format!("inline: {label}"),
            Self::Field(name) => format!("field: {name}"),
        }
    }

    pub fn expand(&self, row: &Row) -> Result<String> {
        match self {
            Self::File { contents, .. } | Self::Inline { contents, .. } => {
                fill_template(contents, row).map_err(|e| anyhow::anyhow!(e.to_string()))
            }
            Self::Field(name) => {
                let v = row
                    .get(name)
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .unwrap_or_default();
                Ok(v)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;
    use serde_json::Value;

    fn row(pairs: &[(&str, &str)]) -> Row {
        let mut r: Row = IndexMap::new();
        for (k, v) in pairs {
            r.insert((*k).to_string(), Value::String((*v).to_string()));
        }
        r
    }

    #[test]
    fn field_source_reads_value() {
        let src = TemplateSource::field("Front".into());
        assert_eq!(src.expand(&row(&[("Front", "hello")])).unwrap(), "hello");
    }

    #[test]
    fn field_source_missing_is_empty() {
        let src = TemplateSource::field("Missing".into());
        assert_eq!(src.expand(&row(&[("Front", "hello")])).unwrap(), "");
    }

    #[test]
    fn file_source_fills_placeholders() {
        let src = TemplateSource::File {
            path: PathBuf::from("-"),
            contents: "{Front} - {Back}".to_string(),
        };
        assert_eq!(
            src.expand(&row(&[("Front", "cat"), ("Back", "feline")]))
                .unwrap(),
            "cat - feline"
        );
    }

    #[test]
    fn inline_source_fills_placeholders() {
        let src = TemplateSource::inline(
            "tts.source.template".to_string(),
            "{front}: {back}".to_string(),
        );
        assert_eq!(
            src.expand(&row(&[("front", "cat"), ("back", "feline")]))
                .unwrap(),
            "cat: feline"
        );
        assert_eq!(src.display_label(), "inline: tts.source.template");
    }
}

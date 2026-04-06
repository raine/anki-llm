use thiserror::Error;

#[derive(Debug, Error)]
pub enum TemplateError {
    #[error("ambiguous keys in row: '{0}' and '{1}' both match when lowercased")]
    AmbiguousKeys(String, String),

    #[error("missing data for template placeholders: {0}")]
    MissingPlaceholders(String),
}

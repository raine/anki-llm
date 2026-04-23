use thiserror::Error;

#[derive(Debug, Error)]
pub enum TemplateError {
    #[error("ambiguous keys in row: '{0}' and '{1}' both match when lowercased")]
    AmbiguousKeys(String, String),

    #[error("missing data for template placeholders: {0}")]
    MissingPlaceholders(String),

    #[error("invalid frontmatter: {0}")]
    InvalidFrontmatter(String),

    #[error("invalid prompt: {0}")]
    InvalidPrompt(String),
}

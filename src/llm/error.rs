use thiserror::Error;

#[derive(Debug, Error)]
pub enum LlmError {
    #[error("LLM request failed: {0}")]
    Http(String),

    #[error("failed to decode LLM response: {0}")]
    Decode(String),

    #[error("LLM API error: {0}")]
    Api(String),
}

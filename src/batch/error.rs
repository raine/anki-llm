use thiserror::Error;

#[derive(Debug, Error)]
pub enum BatchError {
    #[error("{0}")]
    Processing(String),

    /// Non-retryable errors (template issues, config problems).
    #[error("{0}")]
    Fatal(String),
}

impl BatchError {
    /// Whether this error should be retried.
    pub fn is_retryable(&self) -> bool {
        matches!(self, BatchError::Processing(_))
    }
}

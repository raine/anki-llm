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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn processing_is_retryable() {
        assert!(BatchError::Processing("timeout".into()).is_retryable());
    }

    #[test]
    fn fatal_is_not_retryable() {
        assert!(!BatchError::Fatal("bad template".into()).is_retryable());
    }
}

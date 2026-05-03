use thiserror::Error;

/// Error taxonomy for the TTS subsystem. Mirrors the LLM subsystem's split:
/// transient errors get retried by the batch engine, permanent ones do not.
#[derive(Debug, Error)]
pub enum TtsError {
    #[error("{0}")]
    Transient(String),
    #[error("{0}")]
    Permanent(String),
}

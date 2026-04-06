use thiserror::Error;

#[derive(Debug, Error)]
pub enum AnkiConnectError {
    #[error("could not connect to AnkiConnect at {url}: {source}")]
    Connection {
        url: String,
        #[source]
        source: ureq::Error,
    },

    #[error("failed to decode AnkiConnect response: {0}")]
    Decode(#[source] ureq::Error),

    #[error("AnkiConnect API error: {0}")]
    Api(String),

    #[error("AnkiConnect returned null result for action: {0}")]
    NullResult(String),
}

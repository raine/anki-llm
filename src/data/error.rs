use thiserror::Error;

#[derive(Debug, Error)]
pub enum DataError {
    #[error("unsupported file format: {0}. Use .csv, .yaml, or .yml")]
    UnsupportedFormat(String),

    #[error("CSV parse error: {0}")]
    CsvParse(String),

    #[error("YAML parse error: {0}")]
    YamlParse(#[from] serde_yaml::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[allow(dead_code)]
    #[error("row missing required identifier (noteId, id, or Id). Fields: {0}")]
    MissingNoteId(String),
}

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// AnkiConnect request envelope.
#[derive(Debug, Serialize)]
pub struct AnkiRequest<P: Serialize> {
    pub action: String,
    pub version: u8,
    pub params: P,
}

/// AnkiConnect response envelope.
#[derive(Debug, Deserialize)]
pub struct AnkiResponse<R> {
    pub result: Option<R>,
    pub error: Option<String>,
}

/// A single field in an Anki note.
#[derive(Debug, Clone, Deserialize)]
pub struct NoteField {
    pub value: String,
    pub order: u32,
}

/// Full note info returned by `notesInfo`.
/// Uses `IndexMap` to preserve Anki's field order.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteInfo {
    pub note_id: i64,
    pub fields: IndexMap<String, NoteField>,
    pub tags: Vec<String>,
    pub model_name: String,
}

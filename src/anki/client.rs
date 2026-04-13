use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::anki::error::AnkiConnectError;
use crate::anki::schema::{AddNoteParams, AnkiRequest, AnkiResponse, NoteInfo};

pub const DEFAULT_URL: &str = "http://127.0.0.1:8765";

/// Escape a term for use inside a quoted Anki search token (e.g. `deck:"..."`, `note:"..."`).
/// Backslashes and double-quotes must be escaped so the query parser handles them correctly.
pub fn anki_quote(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}
const API_VERSION: u8 = 6;

pub struct AnkiClient {
    url: String,
}

impl Default for AnkiClient {
    fn default() -> Self {
        Self::new()
    }
}

impl AnkiClient {
    pub fn new() -> Self {
        Self {
            url: DEFAULT_URL.to_string(),
        }
    }

    /// Create a client pointing at a custom URL (for testing with mock servers).
    pub fn with_url(url: impl Into<String>) -> Self {
        Self { url: url.into() }
    }

    /// Send a typed request and deserialize the result.
    pub fn request<P: Serialize, R: DeserializeOwned>(
        &self,
        action: &str,
        params: P,
    ) -> Result<R, AnkiConnectError> {
        let req = AnkiRequest {
            action: action.to_string(),
            version: API_VERSION,
            params,
        };

        let body = serde_json::to_string(&req).expect("request serialization should not fail");

        let mut http_response = ureq::post(&self.url)
            .header("Content-Type", "application/json")
            .send(body.as_bytes())
            .map_err(|e| AnkiConnectError::Connection {
                url: self.url.clone(),
                source: e,
            })?;

        let response: AnkiResponse<R> = http_response
            .body_mut()
            .read_json()
            .map_err(AnkiConnectError::Decode)?;

        if let Some(err) = response.error {
            return Err(AnkiConnectError::Api(err));
        }

        response
            .result
            .ok_or_else(|| AnkiConnectError::NullResult(action.to_string()))
    }

    /// Send a request with no params and deserialize the result.
    pub fn request_no_params<R: DeserializeOwned>(
        &self,
        action: &str,
    ) -> Result<R, AnkiConnectError> {
        self.request(action, serde_json::json!({}))
    }

    /// Send a request for an action that returns null on success (e.g. deleteNotes).
    /// Only checks for API errors; ignores the result value.
    pub fn request_void<P: Serialize>(
        &self,
        action: &str,
        params: P,
    ) -> Result<(), AnkiConnectError> {
        #[derive(serde::Deserialize)]
        struct ErrorOnly {
            error: Option<String>,
        }

        let req = AnkiRequest {
            action: action.to_string(),
            version: API_VERSION,
            params,
        };
        let body = serde_json::to_string(&req).expect("request serialization should not fail");
        let mut http_response = ureq::post(&self.url)
            .header("Content-Type", "application/json")
            .send(body.as_bytes())
            .map_err(|e| AnkiConnectError::Connection {
                url: self.url.clone(),
                source: e,
            })?;
        let response: ErrorOnly = http_response
            .body_mut()
            .read_json()
            .map_err(AnkiConnectError::Decode)?;
        if let Some(err) = response.error {
            return Err(AnkiConnectError::Api(err));
        }
        Ok(())
    }

    /// Send a raw request, returning the result as an untyped `serde_json::Value`.
    pub fn request_raw(
        &self,
        action: &str,
        params: Option<Value>,
    ) -> Result<Value, AnkiConnectError> {
        let params = params.unwrap_or(Value::Object(serde_json::Map::new()));
        self.request(action, params)
    }

    pub fn deck_names(&self) -> Result<Vec<String>, AnkiConnectError> {
        self.request_no_params("deckNames")
    }

    pub fn model_names(&self) -> Result<Vec<String>, AnkiConnectError> {
        self.request_no_params("modelNames")
    }

    pub fn model_field_names(&self, model_name: &str) -> Result<Vec<String>, AnkiConnectError> {
        self.request(
            "modelFieldNames",
            serde_json::json!({ "modelName": model_name }),
        )
    }

    pub fn find_notes(&self, query: &str) -> Result<Vec<i64>, AnkiConnectError> {
        self.request("findNotes", serde_json::json!({ "query": query }))
    }

    pub fn notes_info(&self, notes: &[i64]) -> Result<Vec<NoteInfo>, AnkiConnectError> {
        self.request("notesInfo", serde_json::json!({ "notes": notes }))
    }

    /// Find the model name used by the first note in a deck.
    /// Returns None if the deck is empty.
    pub fn find_model_name_for_deck(
        &self,
        deck_name: &str,
    ) -> Result<Option<String>, AnkiConnectError> {
        let note_ids = self.find_notes(&format!("deck:\"{deck_name}\""))?;
        if note_ids.is_empty() {
            return Ok(None);
        }
        let notes = self.notes_info(&note_ids[..1])?;
        Ok(notes.into_iter().next().map(|n| n.model_name))
    }

    /// Find all unique model names used in a deck, sorted alphabetically.
    pub fn find_model_names_for_deck(
        &self,
        deck_name: &str,
    ) -> Result<Vec<String>, AnkiConnectError> {
        let note_ids = self.find_notes(&format!("deck:\"{deck_name}\""))?;
        if note_ids.is_empty() {
            return Ok(Vec::new());
        }
        let notes = self.notes_info(&note_ids)?;
        let names: Vec<String> = notes
            .into_iter()
            .map(|n| n.model_name)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        Ok(names)
    }

    /// Add notes to Anki. Returns a vec of note IDs (None for failures).
    pub fn add_notes(&self, notes: &[AddNoteParams]) -> Result<Vec<Option<i64>>, AnkiConnectError> {
        self.request("addNotes", serde_json::json!({ "notes": notes }))
    }

    /// Execute multiple actions in a single request.
    pub fn multi(
        &self,
        actions: &[serde_json::Value],
    ) -> Result<Vec<serde_json::Value>, AnkiConnectError> {
        self.request("multi", serde_json::json!({ "actions": actions }))
    }

    /// Delete notes by their IDs.
    pub fn delete_notes(&self, notes: &[i64]) -> Result<(), AnkiConnectError> {
        self.request_void("deleteNotes", serde_json::json!({ "notes": notes }))
    }

    /// Delete decks by name. If `cards_too` is true, also deletes all cards in those decks.
    pub fn delete_decks(&self, decks: &[&str], cards_too: bool) -> Result<(), AnkiConnectError> {
        self.request_void(
            "deleteDecks",
            serde_json::json!({ "decks": decks, "cardsToo": cards_too }),
        )
    }

    /// Create a new empty deck. Returns the deck ID.
    pub fn create_deck(&self, deck_name: &str) -> Result<i64, AnkiConnectError> {
        self.request("createDeck", serde_json::json!({ "deck": deck_name }))
    }

    /// Get the list of Anki profile names.
    pub fn get_profiles(&self) -> Result<Vec<String>, AnkiConnectError> {
        self.request_no_params("getProfiles")
    }

    /// Switch to a different Anki profile.
    pub fn load_profile(&self, name: &str) -> Result<bool, AnkiConnectError> {
        self.request("loadProfile", serde_json::json!({ "name": name }))
    }

    /// Upload a media file to Anki's collection.media directory via
    /// AnkiConnect's `storeMediaFile` action. Returns the stored filename
    /// (AnkiConnect may adjust it if a same-named file already exists).
    pub fn store_media_file(
        &self,
        filename: &str,
        data: &[u8],
    ) -> Result<String, AnkiConnectError> {
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(data);
        self.request(
            "storeMediaFile",
            serde_json::json!({ "filename": filename, "data": b64 }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anki_quote_simple() {
        assert_eq!(anki_quote("My Deck"), "\"My Deck\"");
    }

    #[test]
    fn anki_quote_with_double_quotes() {
        assert_eq!(anki_quote(r#"Foo "Bar""#), r#""Foo \"Bar\"""#);
    }

    #[test]
    fn anki_quote_with_backslash() {
        assert_eq!(anki_quote(r"path\to"), r#""path\\to""#);
    }
}

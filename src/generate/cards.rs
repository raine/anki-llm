use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use indexmap::IndexMap;

use crate::anki::client::AnkiClient;
use crate::template::frontmatter::Frontmatter;

use super::processor::CardCandidate;

/// Monotonic counter minting stable `ValidatedCard` ids. Used by the
/// TUI to route async per-card state (TTS preview, future edits) across
/// regeneration without relying on fragile selection indices.
static CARD_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

pub fn next_card_id() -> u64 {
    CARD_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// A card with mapped Anki fields and duplicate status.
#[derive(Clone)]
pub struct ValidatedCard {
    /// Stable identifier for per-card async state routing (TTS preview
    /// status, etc.). Re-minted on regeneration so old state for the
    /// replaced card is implicitly invalidated.
    pub card_id: u64,
    /// Original fields (LLM keys → values as strings after sanitization).
    pub fields: HashMap<String, String>,
    /// Fields mapped to Anki field names (sanitized HTML, for Anki import).
    pub anki_fields: IndexMap<String, String>,
    /// Fields mapped to Anki field names (raw markdown, for terminal display).
    pub raw_anki_fields: IndexMap<String, String>,
    /// Whether this card already exists in Anki.
    pub is_duplicate: bool,
    /// Note ID of the existing duplicate in Anki, if any.
    pub duplicate_note_id: Option<i64>,
    /// Fields of the existing duplicate note in Anki (field name → value).
    pub duplicate_fields: Option<IndexMap<String, String>>,
    /// Informational flags from pre-select check steps.
    pub flags: Vec<String>,
    /// LLM model used to generate this card.
    pub model: String,
}

/// Map card fields from LLM keys to Anki field names.
pub fn map_fields_to_anki(
    sanitized: &HashMap<String, String>,
    field_map: &IndexMap<String, String>,
) -> Result<IndexMap<String, String>, anyhow::Error> {
    let mut anki_fields = IndexMap::new();
    for (llm_key, anki_name) in field_map {
        let value = sanitized
            .get(llm_key)
            .ok_or_else(|| anyhow::anyhow!("Missing field \"{llm_key}\" in card"))?;
        anki_fields.insert(anki_name.clone(), value.clone());
    }
    Ok(anki_fields)
}

/// Escape special characters for Anki search queries.
pub fn escape_anki_query(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('*', "\\*")
        .replace('_', "\\_")
}

/// Check if a note with this first field value already exists.
/// Returns the note ID of the first match, or None if no duplicate found.
fn check_duplicate(
    anki: &AnkiClient,
    first_field_value: &str,
    note_type: &str,
    deck: &str,
) -> Result<Option<i64>, anyhow::Error> {
    let escaped = escape_anki_query(first_field_value);
    let query = format!("\"note:{note_type}\" \"deck:{deck}\" \"{escaped}\"");
    let ids = anki.find_notes(&query)?;
    Ok(ids.into_iter().next())
}

/// Fetch the fields of an existing note by its ID.
fn fetch_note_fields(
    anki: &AnkiClient,
    note_id: i64,
) -> Result<IndexMap<String, String>, anyhow::Error> {
    let notes = anki.notes_info(&[note_id])?;
    let note = notes
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("Note {note_id} not found"))?;
    Ok(note.fields.into_iter().map(|(k, v)| (k, v.value)).collect())
}

/// The full duplicate-metadata shape returned by
/// `lookup_duplicate_metadata` — paired `note_id` + existing-note
/// fields so callers can populate `ValidatedCard.duplicate_note_id` /
/// `duplicate_fields` in one shot.
pub(super) type DuplicateMetadata = (Option<i64>, Option<IndexMap<String, String>>);

/// Resolve the full duplicate-metadata shape (`duplicate_note_id` +
/// `duplicate_fields`) for a given first-field value. Used by both
/// freshly generated cards (`build_validated_card`) and the TUI's
/// post-`$EDITOR` duplicate refresh — previously the edit path only
/// recomputed `is_duplicate` and left the diff panel rendering against
/// stale (or missing) metadata.
pub(super) fn lookup_duplicate_metadata(
    anki: &AnkiClient,
    first_field_value: &str,
    note_type: &str,
    deck: &str,
) -> Result<DuplicateMetadata, anyhow::Error> {
    if first_field_value.is_empty() {
        return Ok((None, None));
    }
    let dup_note_id = check_duplicate(anki, first_field_value, note_type, deck)?;
    let duplicate_fields = dup_note_id.and_then(|id| fetch_note_fields(anki, id).ok());
    Ok((dup_note_id, duplicate_fields))
}

/// Build a fully-populated `ValidatedCard` from already-sanitized LLM-keyed
/// fields and the corresponding raw (pre-sanitization) strings. Handles the
/// field-map projection, the duplicate check against Anki's first-field value,
/// and the on-duplicate fetch of the existing note's fields — the same
/// sequence `validate_cards` uses for freshly generated cards, exposed so
/// `regenerate_single_card` can reuse it and end up with populated
/// `duplicate_fields` and `model` instead of the placeholder values its
/// inline constructor used to hardcode.
pub(super) fn build_validated_card(
    sanitized: HashMap<String, String>,
    raw_strings: &HashMap<String, String>,
    frontmatter: &Frontmatter,
    first_field_name: &str,
    anki: &AnkiClient,
    model: &str,
) -> Result<ValidatedCard, anyhow::Error> {
    let anki_fields = map_fields_to_anki(&sanitized, &frontmatter.field_map)?;
    let raw_anki_fields = map_fields_to_anki(raw_strings, &frontmatter.field_map)?;

    let first_field_value = anki_fields
        .get(first_field_name)
        .map(String::as_str)
        .unwrap_or("");
    let (dup_note_id, duplicate_fields) = lookup_duplicate_metadata(
        anki,
        first_field_value,
        &frontmatter.note_type,
        &frontmatter.deck,
    )?;

    Ok(ValidatedCard {
        card_id: next_card_id(),
        fields: sanitized,
        anki_fields,
        raw_anki_fields,
        is_duplicate: dup_note_id.is_some(),
        duplicate_note_id: dup_note_id,
        duplicate_fields,
        flags: Vec::new(),
        model: model.to_string(),
    })
}

/// Validate cards: map fields to Anki names and check for duplicates.
pub fn validate_cards(
    cards: Vec<(CardCandidate, HashMap<String, String>)>,
    frontmatter: &Frontmatter,
    first_field_name: &str,
    anki: &AnkiClient,
    model: &str,
) -> Result<Vec<ValidatedCard>, anyhow::Error> {
    let mut validated = Vec::new();
    for (candidate, sanitized) in cards {
        // Build raw (pre-sanitization) field strings for terminal display.
        let raw_strings: HashMap<String, String> = candidate
            .fields
            .iter()
            .map(|(k, v)| {
                let s = match v {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                (k.clone(), s)
            })
            .collect();

        validated.push(build_validated_card(
            sanitized,
            &raw_strings,
            frontmatter,
            first_field_name,
            anki,
            model,
        )?);
    }
    Ok(validated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn escape_special_chars() {
        assert_eq!(
            escape_anki_query(r#"hello "world" foo_bar"#),
            r#"hello \"world\" foo\_bar"#
        );
    }

    #[test]
    fn map_fields() {
        let mut sanitized = HashMap::new();
        sanitized.insert("front".into(), "hello".into());
        sanitized.insert("back".into(), "world".into());

        let mut field_map = IndexMap::new();
        field_map.insert("front".into(), "Front".into());
        field_map.insert("back".into(), "Back".into());

        let result = map_fields_to_anki(&sanitized, &field_map).unwrap();
        assert_eq!(result["Front"], "hello");
        assert_eq!(result["Back"], "world");
    }

    /// Spawn a minimal AnkiConnect mock on an ephemeral loopback port.
    /// Serves the JSON responses in `responses` one per accepted
    /// connection, in order, matching the request order the helper
    /// makes (findNotes, then notesInfo). Parses request headers,
    /// respects `Content-Length`, and drains the request body before
    /// replying so the HTTP client never sees an early socket close
    /// (which would surface as `ECONNRESET`/`EPIPE` if the body
    /// arrived in a separate TCP segment).
    ///
    /// The returned worker thread is detached — the caller must not
    /// `join()` it. If the test finishes after serving fewer requests
    /// than expected (e.g. an early error path), the thread stays
    /// blocked in `accept()` and is reaped when the process exits;
    /// `join()`ing in that case would deadlock the test.
    fn spawn_mock_anki(responses: Vec<&'static str>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://{addr}");
        thread::spawn(move || {
            for body in responses {
                let Ok((stream, _)) = listener.accept() else {
                    return;
                };
                let mut reader = BufReader::new(stream);
                // Parse the request line + headers until the blank
                // line terminator. We only care about Content-Length;
                // everything else gets discarded.
                let mut content_length: usize = 0;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        break;
                    }
                    if line == "\r\n" || line == "\n" {
                        break;
                    }
                    if let Some(rest) = line
                        .strip_prefix("Content-Length:")
                        .or_else(|| line.strip_prefix("content-length:"))
                    {
                        content_length = rest.trim().parse().unwrap_or(0);
                    }
                }
                // Drain exactly the body bytes so the client's write
                // half has fully landed before we reply.
                if content_length > 0 {
                    let mut body_buf = vec![0u8; content_length];
                    let _ = reader.read_exact(&mut body_buf);
                }
                // Reclaim the underlying stream for writing.
                let mut stream = reader.into_inner();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.shutdown(std::net::Shutdown::Write);
            }
        });
        url
    }

    #[test]
    fn build_validated_card_populates_duplicate_fields_and_model() {
        // Mock AnkiConnect: first findNotes returns a single hit; then
        // notesInfo returns that note's fields. This is exactly the
        // sequence `build_validated_card` goes through on a duplicate.
        let find_notes_body = r#"{"result":[12345],"error":null}"#;
        let notes_info_body = r#"{"result":[{"noteId":12345,"tags":[],"fields":{"Front":{"value":"日本語","order":0},"Back":{"value":"japanese (existing)","order":1}},"modelName":"Basic","cards":[1]}],"error":null}"#;
        let url = spawn_mock_anki(vec![find_notes_body, notes_info_body]);

        let anki = AnkiClient::with_url(&url);
        let mut sanitized: HashMap<String, String> = HashMap::new();
        sanitized.insert("front".into(), "日本語".into());
        sanitized.insert("back".into(), "japanese (regenerated)".into());
        let raw_strings = sanitized.clone();

        let mut field_map: IndexMap<String, String> = IndexMap::new();
        field_map.insert("front".into(), "Front".into());
        field_map.insert("back".into(), "Back".into());
        let frontmatter = Frontmatter {
            title: None,
            description: None,
            deck: "Test".into(),
            note_type: "Basic".into(),
            field_map,
            processing: None,
            tts: None,
        };

        let card = build_validated_card(
            sanitized,
            &raw_strings,
            &frontmatter,
            "Front",
            &anki,
            "gpt-test",
        )
        .unwrap();

        assert!(card.is_duplicate, "should flag the card as a duplicate");
        assert_eq!(card.duplicate_note_id, Some(12345));
        let dup_fields = card
            .duplicate_fields
            .as_ref()
            .expect("duplicate_fields must be populated on duplicate hit");
        assert_eq!(dup_fields.get("Front").map(String::as_str), Some("日本語"));
        assert_eq!(
            dup_fields.get("Back").map(String::as_str),
            Some("japanese (existing)"),
            "duplicate_fields must carry the existing Anki note's values, not the regenerated card's"
        );
        assert_eq!(
            card.model, "gpt-test",
            "model label must survive regeneration for multi-model sessions"
        );
    }
}

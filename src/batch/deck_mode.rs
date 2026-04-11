use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Context;
use indexmap::IndexMap;
use serde_json::{Value, json};

use crate::anki::client::AnkiClient;
use crate::data::rows::Row;
use crate::snapshot::store::NoteRevision;

use super::report::RowOutcome;

/// Internal key used to store the Anki note ID in a row.
/// Starts with `_` so it is automatically excluded from field updates.
pub const ANKI_NOTE_ID_KEY: &str = "__note_id";

/// Queues Anki updateNoteFields actions and flushes them in batches via `multi`.
/// Tracks flush errors so the command can report accurate success/failure.
pub struct DeckWriter {
    anki: AnkiClient,
    /// Pending updateNoteFields actions to flush.
    queue: Mutex<Vec<Value>>,
    /// Flush threshold (typically batch_size).
    flush_threshold: usize,
    /// Path to error log (JSONL).
    error_log_path: PathBuf,
    /// Number of successful Anki updates.
    success_count: Mutex<usize>,
    /// Set to true if any Anki flush fails — prevents further writes.
    pub has_flush_error: AtomicBool,
    /// Original field values keyed by note_id, for snapshot diffing.
    before_fields: IndexMap<i64, IndexMap<String, String>>,
    /// Collected note revisions from successful updates.
    revisions: Mutex<Vec<NoteRevision>>,
}

impl DeckWriter {
    pub fn new(
        anki: AnkiClient,
        flush_threshold: usize,
        error_log_path: PathBuf,
        before_fields: IndexMap<i64, IndexMap<String, String>>,
    ) -> anyhow::Result<Self> {
        // Truncate error log from prior runs. Fail early if the path is unwritable.
        File::create(&error_log_path)
            .with_context(|| format!("failed to create error log: {}", error_log_path.display()))?;

        Ok(Self {
            anki,
            queue: Mutex::new(Vec::new()),
            flush_threshold,
            error_log_path,
            success_count: Mutex::new(0),
            has_flush_error: AtomicBool::new(false),
            before_fields,
            revisions: Mutex::new(Vec::new()),
        })
    }

    /// Record a completed row outcome. Queues Anki updates for successes,
    /// logs failures to the error JSONL file.
    pub fn on_row_done(&self, outcome: &RowOutcome) {
        // Skip further Anki writes if a flush already failed
        if self.has_flush_error.load(Ordering::Relaxed) {
            return;
        }

        match outcome {
            RowOutcome::Success(row) => {
                if let Some(action) = build_update_action(row) {
                    // Record the note revision for snapshot
                    self.record_revision(row);

                    let should_flush = {
                        let mut queue = self.queue.lock().unwrap();
                        queue.push(action);
                        queue.len() >= self.flush_threshold
                    };
                    if should_flush && let Err(e) = self.flush() {
                        eprintln!("Error: failed to flush Anki updates: {e}");
                    }
                }
            }
            RowOutcome::Failure { row, error } => {
                self.append_error_log(row, error);
            }
        }
    }

    /// Extract the note ID from a row.
    fn extract_note_id(row: &Row) -> Option<i64> {
        row.get(ANKI_NOTE_ID_KEY).and_then(|v| match v {
            Value::Number(n) => n.as_i64(),
            Value::String(s) => s.parse().ok(),
            _ => None,
        })
    }

    /// Record a note revision by diffing the updated row against stored before_fields.
    fn record_revision(&self, row: &Row) {
        let Some(note_id) = Self::extract_note_id(row) else {
            return;
        };
        let Some(before) = self.before_fields.get(&note_id) else {
            return;
        };

        // Compute sparse after_fields: only fields that changed
        let mut after_fields = IndexMap::new();
        for (key, value) in row {
            if key.starts_with('_') {
                continue;
            }
            let current = match value {
                Value::String(s) => s.replace('\r', ""),
                Value::Null => String::new(),
                other => other.to_string(),
            };
            if let Some(original) = before.get(key)
                && &current != original
            {
                after_fields.insert(key.clone(), current);
            }
        }

        // Only record if something actually changed
        if !after_fields.is_empty() {
            self.revisions.lock().unwrap().push(NoteRevision {
                note_id,
                before_fields: before.clone(),
                after_fields,
            });
        }
    }

    /// Take the collected revisions out of the writer.
    pub fn take_revisions(&self) -> Vec<NoteRevision> {
        std::mem::take(&mut *self.revisions.lock().unwrap())
    }

    /// Flush all queued updates to Anki via `multi`.
    pub fn flush(&self) -> anyhow::Result<()> {
        let actions: Vec<Value> = {
            let mut queue = self.queue.lock().unwrap();
            if queue.is_empty() {
                return Ok(());
            }
            std::mem::take(&mut *queue)
        };

        let count = actions.len();
        let results = self.anki.multi(&actions).inspect_err(|_e| {
            self.has_flush_error.store(true, Ordering::SeqCst);
        })?;

        // Check for individual failures (updateNoteFields returns null on success)
        let failures: Vec<_> = results
            .iter()
            .enumerate()
            .filter(|(_, r)| !r.is_null())
            .collect();

        if !failures.is_empty() {
            self.has_flush_error.store(true, Ordering::SeqCst);
            anyhow::bail!(
                "{} of {} Anki update operations failed",
                failures.len(),
                count
            );
        }

        *self.success_count.lock().unwrap() += count;
        Ok(())
    }

    pub fn success_count(&self) -> usize {
        *self.success_count.lock().unwrap()
    }

    fn append_error_log(&self, row: &Row, error: &str) {
        let entry = json!({ "error": error, "note": row });
        let line = serde_json::to_string(&entry).unwrap_or_default();
        let result = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.error_log_path)
            .and_then(|mut f| writeln!(f, "{line}"));
        if let Err(e) = result {
            eprintln!("Warning: failed to write error log: {e}");
        }
    }
}

/// Build an `updateNoteFields` action for the `multi` endpoint.
/// Returns None if the row has no internal note ID key.
fn build_update_action(row: &Row) -> Option<Value> {
    let id: i64 = row.get(ANKI_NOTE_ID_KEY).and_then(|v| match v {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    })?;

    let mut fields = serde_json::Map::new();
    for (key, value) in row {
        // Skip internal fields (prefixed with _); ANKI_NOTE_ID_KEY starts with _
        if key.starts_with('_') {
            continue;
        }
        // Convert value to string for Anki
        let s = match value {
            Value::String(s) => s.clone(),
            Value::Null => String::new(),
            other => other.to_string(),
        };
        fields.insert(key.clone(), Value::String(s));
    }

    Some(json!({
        "action": "updateNoteFields",
        "params": {
            "note": {
                "id": id,
                "fields": fields
            }
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;

    #[test]
    fn build_update_action_basic() {
        let mut row: Row = IndexMap::new();
        row.insert(ANKI_NOTE_ID_KEY.into(), Value::from(12345_i64));
        row.insert("Front".into(), Value::from("hello"));
        row.insert("Back".into(), Value::from("world"));

        let action = build_update_action(&row).unwrap();
        let params = &action["params"]["note"];
        assert_eq!(params["id"], 12345);
        assert_eq!(params["fields"]["Front"], "hello");
        assert_eq!(params["fields"]["Back"], "world");
        assert!(params["fields"].get(ANKI_NOTE_ID_KEY).is_none());
    }

    #[test]
    fn build_update_action_skips_internal_fields() {
        let mut row: Row = IndexMap::new();
        row.insert(ANKI_NOTE_ID_KEY.into(), Value::from(1_i64));
        row.insert("_error".into(), Value::from("oops"));
        row.insert("Front".into(), Value::from("x"));

        let action = build_update_action(&row).unwrap();
        assert!(action["params"]["note"]["fields"].get("_error").is_none());
    }

    #[test]
    fn build_update_action_allows_real_id_field() {
        // A real Anki field named "id" should be updated, not silently dropped
        let mut row: Row = IndexMap::new();
        row.insert(ANKI_NOTE_ID_KEY.into(), Value::from(42_i64));
        row.insert("id".into(), Value::from("some-value"));

        let action = build_update_action(&row).unwrap();
        assert_eq!(action["params"]["note"]["fields"]["id"], "some-value");
    }
}

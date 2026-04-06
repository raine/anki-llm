use std::path::PathBuf;
use std::sync::Mutex;

use indexmap::IndexMap;

use crate::data::io::{atomic_write_file, serialize_rows};
use crate::data::rows::{Row, get_note_id};

use super::report::RowOutcome;

/// Maintains output state and writes incrementally to disk.
pub struct FileWriter {
    /// Path to the output file.
    output_path: PathBuf,
    /// All rows keyed by note ID (insertion order doesn't matter — we use
    /// ordered_ids for output ordering).
    all_rows: Mutex<IndexMap<String, Row>>,
    /// Original input row IDs in order — used to preserve row order on flush.
    ordered_ids: Vec<String>,
    /// Counter for unflushed updates; flush when >= threshold.
    pending: Mutex<usize>,
    /// Flush threshold (typically batch_size).
    flush_threshold: usize,
    /// Serializes concurrent flush calls so that a slower older snapshot
    /// cannot overwrite a newer one written by a concurrent flush.
    flush_lock: Mutex<()>,
}

impl FileWriter {
    /// Create a new file writer.
    ///
    /// `ordered_ids` is the list of all row IDs in input order.
    /// `existing` is pre-loaded output from a previous run (for resume).
    pub fn new(
        output_path: PathBuf,
        ordered_ids: Vec<String>,
        existing: IndexMap<String, Row>,
        flush_threshold: usize,
    ) -> Self {
        Self {
            output_path,
            all_rows: Mutex::new(existing),
            ordered_ids,
            pending: Mutex::new(0),
            flush_threshold,
            flush_lock: Mutex::new(()),
        }
    }

    /// Record a completed row and flush to disk if threshold reached.
    pub fn on_row_done(&self, outcome: &RowOutcome) {
        let row = match outcome {
            RowOutcome::Success(row) => row,
            RowOutcome::Failure { row, .. } => row,
        };

        let id = get_note_id(row).unwrap_or_default();
        if id.is_empty() {
            return;
        }

        {
            let mut all = self.all_rows.lock().unwrap();
            all.insert(id, row.clone());
        }

        let should_flush = {
            let mut pending = self.pending.lock().unwrap();
            *pending += 1;
            if *pending >= self.flush_threshold {
                *pending = 0;
                true
            } else {
                false
            }
        };

        if should_flush && let Err(e) = self.flush() {
            eprintln!("Warning: failed to flush output: {e}");
        }
    }

    /// Number of rows currently stored.
    #[cfg(test)]
    fn row_count(&self) -> usize {
        self.all_rows.lock().unwrap().len()
    }

    /// Write all rows to disk in original input order. Called at end of
    /// processing and periodically during processing.
    pub fn flush(&self) -> anyhow::Result<()> {
        // Serialize flushes so that a slower concurrent flush cannot overwrite
        // a newer snapshot written by a flush that started after it.
        let _flush_guard = self.flush_lock.lock().unwrap();

        // Snapshot rows under the lock, then release before doing I/O so
        // worker threads are not blocked during serialization and disk writes.
        let rows: Vec<Row> = {
            let all = self.all_rows.lock().unwrap();
            if all.is_empty() {
                return Ok(());
            }
            self.ordered_ids
                .iter()
                .filter_map(|id| all.get(id).cloned())
                .collect()
        };

        if rows.is_empty() {
            return Ok(());
        }
        let content = serialize_rows(&rows, &self.output_path)?;
        atomic_write_file(&self.output_path, &content)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    fn make_row(id: i64, front: &str) -> Row {
        let mut row = Row::new();
        row.insert("noteId".into(), json!(id));
        row.insert("Front".into(), json!(front));
        row
    }

    #[test]
    fn flush_writes_rows_in_input_order() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("out.yaml");
        let ids = vec!["1".into(), "2".into(), "3".into()];

        let writer = FileWriter::new(path.clone(), ids, IndexMap::new(), 100);

        // Insert out of order
        writer.on_row_done(&RowOutcome::Success(make_row(3, "third")));
        writer.on_row_done(&RowOutcome::Success(make_row(1, "first")));
        writer.on_row_done(&RowOutcome::Success(make_row(2, "second")));
        writer.flush().unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let first_pos = content.find("first").unwrap();
        let second_pos = content.find("second").unwrap();
        let third_pos = content.find("third").unwrap();
        assert!(first_pos < second_pos);
        assert!(second_pos < third_pos);
    }

    #[test]
    fn flush_threshold_triggers_write() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("out.yaml");
        let ids = vec!["1".into(), "2".into()];

        let writer = FileWriter::new(path.clone(), ids, IndexMap::new(), 2);

        writer.on_row_done(&RowOutcome::Success(make_row(1, "a")));
        assert!(!path.exists(), "should not flush after 1 row (threshold=2)");

        writer.on_row_done(&RowOutcome::Success(make_row(2, "b")));
        assert!(path.exists(), "should flush after 2 rows (threshold=2)");
    }

    #[test]
    fn rows_without_id_are_skipped() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("out.yaml");

        let writer = FileWriter::new(path, vec![], IndexMap::new(), 100);

        let mut row = Row::new();
        row.insert("Front".into(), json!("no id"));
        writer.on_row_done(&RowOutcome::Success(row));

        assert_eq!(writer.row_count(), 0);
    }

    #[test]
    fn existing_rows_are_preserved() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("out.yaml");
        let ids = vec!["1".into(), "2".into()];

        let mut existing = IndexMap::new();
        existing.insert("1".to_string(), make_row(1, "existing"));

        let writer = FileWriter::new(path.clone(), ids, existing, 100);
        writer.on_row_done(&RowOutcome::Success(make_row(2, "new")));
        writer.flush().unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("existing"));
        assert!(content.contains("new"));
    }

    #[test]
    fn failure_rows_are_stored() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("out.yaml");
        let ids = vec!["1".into()];

        let writer = FileWriter::new(path.clone(), ids, IndexMap::new(), 100);

        let mut row = make_row(1, "failed");
        row.insert("_error".into(), json!("some error"));
        writer.on_row_done(&RowOutcome::Failure {
            row,
            error: "some error".into(),
        });
        writer.flush().unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("_error"));
    }
}

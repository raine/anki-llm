#![cfg(feature = "integration")]
//! Integration tests that require a running Anki instance with AnkiConnect.
//!
//! These tests create temporary decks, add notes, exercise the export/import
//! pipeline, then clean up. Use the Docker container for isolation:
//!
//!   just test-integration
//!
//! Or manually:
//!   docker run --rm -d -p 8765:8765 --name anki-test anki-test
//!   cargo test --test anki_integration --features integration -- --test-threads=1
//!   docker stop anki-test
//!
//! Gated behind the `integration` feature so they never run during
//! `cargo test` or `just check`.

use assert_cmd::Command;
use indexmap::IndexMap;
use predicates::prelude::*;
use serial_test::serial;
use tempfile::tempdir;

const TEST_DECK: &str = "anki-llm-integration-test";
const TEST_MODEL: &str = "Basic";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A guard that creates the test deck on setup and tears it down on drop.
/// Works against any Anki instance (local Dev profile or Docker container).
struct TestDeck {
    client: anki_llm::anki::client::AnkiClient,
}

impl TestDeck {
    fn setup() -> Self {
        let client = anki_llm::anki::client::AnkiClient::new();

        // Clean up any leftover from a previous failed run, then create fresh
        let _ = client.delete_decks(&[TEST_DECK], true);
        client.create_deck(TEST_DECK).expect("create test deck");

        Self { client }
    }

    /// Add a note with Front/Back fields (Basic model).
    fn add_note(&self, front: &str, back: &str) -> i64 {
        let mut fields = IndexMap::new();
        fields.insert("Front".to_string(), front.to_string());
        fields.insert("Back".to_string(), back.to_string());
        let params = anki_llm::anki::schema::AddNoteParams {
            deck_name: TEST_DECK.to_string(),
            model_name: TEST_MODEL.to_string(),
            fields,
            tags: vec!["anki-llm-test".to_string()],
        };
        let results = self
            .client
            .add_notes(&[params])
            .expect("add_notes should succeed");
        results[0].expect("note should have been created")
    }
}

impl Drop for TestDeck {
    fn drop(&mut self) {
        let _ = self.client.delete_decks(&[TEST_DECK], true);
    }
}

// ---------------------------------------------------------------------------
// AnkiClient method tests
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn client_deck_names() {
    let deck = TestDeck::setup();
    let names = deck.client.deck_names().unwrap();
    assert!(names.contains(&TEST_DECK.to_string()));
}

#[test]
#[serial]
fn client_model_field_names() {
    let deck = TestDeck::setup();
    let fields = deck.client.model_field_names(TEST_MODEL).unwrap();
    assert!(fields.contains(&"Front".to_string()));
    assert!(fields.contains(&"Back".to_string()));
}

#[test]
#[serial]
fn client_add_find_info_delete() {
    let deck = TestDeck::setup();

    let note_id = deck.add_note("test-front", "test-back");
    assert!(note_id > 0);

    let found = deck
        .client
        .find_notes(&format!("deck:\"{}\"", TEST_DECK))
        .unwrap();
    assert_eq!(found, vec![note_id]);

    let info = deck.client.notes_info(&found).unwrap();
    assert_eq!(info.len(), 1);
    assert_eq!(info[0].fields["Front"].value, "test-front");
    assert_eq!(info[0].fields["Back"].value, "test-back");
    assert_eq!(info[0].model_name, TEST_MODEL);
}

#[test]
#[serial]
fn client_find_model_name_for_deck() {
    let deck = TestDeck::setup();
    deck.add_note("a", "b");

    let name = deck.client.find_model_name_for_deck(TEST_DECK).unwrap();
    assert_eq!(name, Some(TEST_MODEL.to_string()));
}

#[test]
#[serial]
fn client_find_model_name_empty_deck() {
    let deck = TestDeck::setup();
    let name = deck.client.find_model_name_for_deck(TEST_DECK).unwrap();
    assert_eq!(name, None);
}

#[test]
#[serial]
fn client_multi_update() {
    let deck = TestDeck::setup();
    let note_id = deck.add_note("original-front", "original-back");

    let actions = vec![serde_json::json!({
        "action": "updateNoteFields",
        "params": {
            "note": {
                "id": note_id,
                "fields": { "Back": "updated-back" }
            }
        }
    })];
    let results = deck.client.multi(&actions).unwrap();
    assert!(
        results.iter().all(|r| r.is_null()),
        "all updates should succeed"
    );

    let info = deck.client.notes_info(&[note_id]).unwrap();
    assert_eq!(info[0].fields["Back"].value, "updated-back");
}

// ---------------------------------------------------------------------------
// Export command tests (CLI)
// ---------------------------------------------------------------------------

fn anki_cmd() -> Command {
    Command::cargo_bin("anki-llm").unwrap()
}

#[test]
#[serial]
fn export_yaml() {
    let deck = TestDeck::setup();
    let note_id = deck.add_note("export-front", "export-back");

    let tmp = tempdir().unwrap();
    let output = tmp.path().join("out.yaml");

    anki_cmd()
        .args(["export", TEST_DECK, &output.to_string_lossy()])
        .assert()
        .success()
        .stderr(predicate::str::contains("Successfully exported 1 notes"));

    let content = std::fs::read_to_string(&output).unwrap();
    assert!(content.contains("export-front"));
    assert!(content.contains("export-back"));
    assert!(content.contains(&note_id.to_string()));
}

#[test]
#[serial]
fn export_csv() {
    let deck = TestDeck::setup();
    deck.add_note("csv-front", "csv-back");

    let tmp = tempdir().unwrap();
    let output = tmp.path().join("out.csv");

    anki_cmd()
        .args(["export", TEST_DECK, &output.to_string_lossy()])
        .assert()
        .success();

    let content = std::fs::read_to_string(&output).unwrap();
    assert!(content.contains("csv-front"));
    assert!(content.contains("csv-back"));
    assert!(content.contains("noteId"));
}

#[test]
#[serial]
fn export_auto_filename() {
    let deck = TestDeck::setup();
    deck.add_note("auto-front", "auto-back");

    let tmp = tempdir().unwrap();

    anki_cmd()
        .current_dir(tmp.path())
        .args(["export", TEST_DECK])
        .assert()
        .success()
        .stderr(predicate::str::contains("automatically using"));

    let expected = tmp.path().join("anki-llm-integration-test.yaml");
    assert!(expected.exists(), "auto-generated file should exist");
}

#[test]
#[serial]
fn export_empty_deck() {
    let _deck = TestDeck::setup();

    anki_cmd()
        .args(["export", TEST_DECK])
        .assert()
        .success()
        .stderr(predicate::str::contains("No notes found"));
}

#[test]
#[serial]
fn export_multiple_notes_preserves_all() {
    let deck = TestDeck::setup();
    deck.add_note("first", "one");
    deck.add_note("second", "two");
    deck.add_note("third", "three");

    let tmp = tempdir().unwrap();
    let output = tmp.path().join("multi.yaml");

    anki_cmd()
        .args(["export", TEST_DECK, &output.to_string_lossy()])
        .assert()
        .success()
        .stderr(predicate::str::contains("Successfully exported 3 notes"));

    let content = std::fs::read_to_string(&output).unwrap();
    assert!(content.contains("first"));
    assert!(content.contains("second"));
    assert!(content.contains("third"));
}

// ---------------------------------------------------------------------------
// Import command tests (CLI)
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn import_add_new_notes_yaml() {
    let deck = TestDeck::setup();

    let tmp = tempdir().unwrap();
    let input = tmp.path().join("import.yaml");
    std::fs::write(
        &input,
        "- Front: import-one\n  Back: back-one\n- Front: import-two\n  Back: back-two\n",
    )
    .unwrap();

    anki_cmd()
        .args([
            "import",
            &input.to_string_lossy(),
            "-d",
            TEST_DECK,
            "-n",
            TEST_MODEL,
        ])
        .assert()
        .success()
        .stderr(
            predicate::str::contains("2 new notes to add")
                .and(predicate::str::contains("2 succeeded")),
        );

    let found = deck
        .client
        .find_notes(&format!("deck:\"{}\"", TEST_DECK))
        .unwrap();
    assert_eq!(found.len(), 2);
}

#[test]
#[serial]
fn import_add_new_notes_csv() {
    let deck = TestDeck::setup();

    let tmp = tempdir().unwrap();
    let input = tmp.path().join("import.csv");
    std::fs::write(
        &input,
        "Front,Back\ncsv-one,csv-back-one\ncsv-two,csv-back-two\n",
    )
    .unwrap();

    anki_cmd()
        .args([
            "import",
            &input.to_string_lossy(),
            "-d",
            TEST_DECK,
            "-n",
            TEST_MODEL,
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("2 new notes to add"));

    let found = deck
        .client
        .find_notes(&format!("deck:\"{}\"", TEST_DECK))
        .unwrap();
    assert_eq!(found.len(), 2);
}

#[test]
#[serial]
fn import_update_existing_notes_by_note_id() {
    let deck = TestDeck::setup();
    let note_id = deck.add_note("original", "before-update");

    let tmp = tempdir().unwrap();
    let input = tmp.path().join("update.yaml");
    std::fs::write(
        &input,
        format!("- noteId: {note_id}\n  Back: after-update\n"),
    )
    .unwrap();

    anki_cmd()
        .args([
            "import",
            &input.to_string_lossy(),
            "-d",
            TEST_DECK,
            "-n",
            TEST_MODEL,
        ])
        .assert()
        .success()
        .stderr(
            predicate::str::contains("1 existing notes to update")
                .and(predicate::str::contains("updated successfully")),
        );

    let info = deck.client.notes_info(&[note_id]).unwrap();
    assert_eq!(info[0].fields["Back"].value, "after-update");
}

#[test]
#[serial]
fn import_update_existing_notes_by_field_key() {
    let deck = TestDeck::setup();
    deck.add_note("match-me", "old-back");

    let tmp = tempdir().unwrap();
    let input = tmp.path().join("update.yaml");
    std::fs::write(&input, "- Front: match-me\n  Back: new-back\n").unwrap();

    anki_cmd()
        .args([
            "import",
            &input.to_string_lossy(),
            "-d",
            TEST_DECK,
            "-n",
            TEST_MODEL,
            "-k",
            "Front",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("1 existing notes to update"));

    let note_ids = deck
        .client
        .find_notes(&format!("deck:\"{}\"", TEST_DECK))
        .unwrap();
    let info = deck.client.notes_info(&note_ids).unwrap();
    assert_eq!(info[0].fields["Back"].value, "new-back");
}

#[test]
#[serial]
fn import_mixed_add_and_update() {
    let deck = TestDeck::setup();
    let existing_id = deck.add_note("exists", "old-value");

    let tmp = tempdir().unwrap();
    let input = tmp.path().join("mixed.yaml");
    std::fs::write(
        &input,
        format!(
            "- noteId: {existing_id}\n  Back: updated-value\n\
             - noteId: 999999999\n  Front: brand-new\n  Back: brand-new-back\n"
        ),
    )
    .unwrap();

    anki_cmd()
        .args([
            "import",
            &input.to_string_lossy(),
            "-d",
            TEST_DECK,
            "-n",
            TEST_MODEL,
        ])
        .assert()
        .success()
        .stderr(
            predicate::str::contains("1 new notes to add")
                .and(predicate::str::contains("1 existing notes to update")),
        );

    let info = deck.client.notes_info(&[existing_id]).unwrap();
    assert_eq!(info[0].fields["Back"].value, "updated-value");
}

#[test]
#[serial]
fn import_infers_note_type_from_deck() {
    let deck = TestDeck::setup();
    deck.add_note("seed", "seed-back");

    let tmp = tempdir().unwrap();
    let input = tmp.path().join("infer.yaml");
    std::fs::write(&input, "- Front: inferred\n  Back: type-inferred\n").unwrap();

    anki_cmd()
        .args([
            "import",
            &input.to_string_lossy(),
            "-d",
            TEST_DECK,
            "-k",
            "Front",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("Inferred note type: Basic"));
}

#[test]
#[serial]
fn import_rejects_duplicate_keys_in_input() {
    let _deck = TestDeck::setup();

    let tmp = tempdir().unwrap();
    let input = tmp.path().join("dupes.yaml");
    std::fs::write(
        &input,
        "- Front: same\n  Back: one\n- Front: same\n  Back: two\n",
    )
    .unwrap();

    anki_cmd()
        .args([
            "import",
            &input.to_string_lossy(),
            "-d",
            TEST_DECK,
            "-n",
            TEST_MODEL,
            "-k",
            "Front",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("duplicate key"));
}

#[test]
#[serial]
fn import_rejects_invalid_key_field() {
    let _deck = TestDeck::setup();

    let tmp = tempdir().unwrap();
    let input = tmp.path().join("bad-key.yaml");
    std::fs::write(&input, "- Front: hello\n  Back: world\n").unwrap();

    anki_cmd()
        .args([
            "import",
            &input.to_string_lossy(),
            "-d",
            TEST_DECK,
            "-n",
            TEST_MODEL,
            "-k",
            "nonexistent",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found in input file"));
}

#[test]
#[serial]
fn import_rejects_key_field_not_in_model() {
    let _deck = TestDeck::setup();

    let tmp = tempdir().unwrap();
    let input = tmp.path().join("bad-model-key.yaml");
    std::fs::write(
        &input,
        "- external_id: abc\n  Front: hello\n  Back: world\n",
    )
    .unwrap();

    anki_cmd()
        .args([
            "import",
            &input.to_string_lossy(),
            "-d",
            TEST_DECK,
            "-n",
            TEST_MODEL,
            "-k",
            "external_id",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("is not a field on note type"));
}

// ---------------------------------------------------------------------------
// Export → Import round-trip
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn round_trip_yaml() {
    let deck = TestDeck::setup();
    deck.add_note("rt-front-1", "rt-back-1");
    deck.add_note("rt-front-2", "rt-back-2");

    let tmp = tempdir().unwrap();
    let export_path = tmp.path().join("round-trip.yaml");

    // Export
    anki_cmd()
        .args(["export", TEST_DECK, &export_path.to_string_lossy()])
        .assert()
        .success();

    // Delete all notes from deck
    let note_ids = deck
        .client
        .find_notes(&format!("deck:\"{}\"", TEST_DECK))
        .unwrap();
    deck.client.delete_notes(&note_ids).unwrap();

    let remaining = deck
        .client
        .find_notes(&format!("deck:\"{}\"", TEST_DECK))
        .unwrap();
    assert_eq!(remaining.len(), 0, "deck should be empty after delete");

    // Import the exported file — noteIds won't match, so all become adds
    anki_cmd()
        .args([
            "import",
            &export_path.to_string_lossy(),
            "-d",
            TEST_DECK,
            "-n",
            TEST_MODEL,
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("2 new notes to add"));

    // Verify content
    let all_ids = deck
        .client
        .find_notes(&format!("deck:\"{}\"", TEST_DECK))
        .unwrap();
    assert_eq!(all_ids.len(), 2);

    let notes = deck.client.notes_info(&all_ids).unwrap();
    let fronts: Vec<&str> = notes
        .iter()
        .map(|n| n.fields["Front"].value.as_str())
        .collect();
    assert!(fronts.contains(&"rt-front-1"));
    assert!(fronts.contains(&"rt-front-2"));
}

#[test]
#[serial]
fn round_trip_csv() {
    let deck = TestDeck::setup();
    deck.add_note("csv-rt-1", "csv-back-1");

    let tmp = tempdir().unwrap();
    let export_path = tmp.path().join("round-trip.csv");

    // Export as CSV
    anki_cmd()
        .args(["export", TEST_DECK, &export_path.to_string_lossy()])
        .assert()
        .success();

    // Delete notes
    let note_ids = deck
        .client
        .find_notes(&format!("deck:\"{}\"", TEST_DECK))
        .unwrap();
    deck.client.delete_notes(&note_ids).unwrap();

    // Import back
    anki_cmd()
        .args([
            "import",
            &export_path.to_string_lossy(),
            "-d",
            TEST_DECK,
            "-n",
            TEST_MODEL,
        ])
        .assert()
        .success();

    let all_ids = deck
        .client
        .find_notes(&format!("deck:\"{}\"", TEST_DECK))
        .unwrap();
    assert_eq!(all_ids.len(), 1);
    let notes = deck.client.notes_info(&all_ids).unwrap();
    assert_eq!(notes[0].fields["Front"].value, "csv-rt-1");
    assert_eq!(notes[0].fields["Back"].value, "csv-back-1");
}

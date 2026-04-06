use assert_cmd::Command;
use predicates::prelude::*;
use tempfile;

#[test]
fn prints_help_with_all_subcommands() {
    let assert = Command::cargo_bin("anki-llm")
        .unwrap()
        .arg("--help")
        .assert()
        .success();

    let stdout = assert.get_output().stdout.clone();
    let help = String::from_utf8(stdout).unwrap();

    for cmd in [
        "export",
        "import",
        "process-file",
        "process-deck",
        "query",
        "config",
        "generate",
        "generate-init",
    ] {
        assert!(help.contains(cmd), "help output missing command: {cmd}");
    }
}

#[test]
fn prints_version() {
    Command::cargo_bin("anki-llm")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("anki-llm 0.1.0"));
}

#[test]
fn config_path_prints_path() {
    Command::cargo_bin("anki-llm")
        .unwrap()
        .args(["config", "path"])
        .assert()
        .success()
        .stdout(predicate::str::contains(".config/anki-llm/config.json"));
}

#[test]
fn config_list_empty_when_no_file() {
    let tmp = tempfile::tempdir().unwrap();
    Command::cargo_bin("anki-llm")
        .unwrap()
        .env("HOME", tmp.path())
        .args(["config", "list"])
        .assert()
        .success();
}

#[test]
fn config_set_and_get_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    Command::cargo_bin("anki-llm")
        .unwrap()
        .env("HOME", tmp.path())
        .args(["config", "set", "model", "gpt-5-mini"])
        .assert()
        .success();
    Command::cargo_bin("anki-llm")
        .unwrap()
        .env("HOME", tmp.path())
        .args(["config", "get", "model"])
        .assert()
        .success()
        .stdout(predicate::str::contains("gpt-5-mini"));
}

#[test]
fn query_docs_prints_documentation() {
    Command::cargo_bin("anki-llm")
        .unwrap()
        .args(["query", "docs"])
        .assert()
        .success()
        .stdout(predicate::str::contains("AnkiConnect"));
}

#[test]
fn query_invalid_json_fails() {
    Command::cargo_bin("anki-llm")
        .unwrap()
        .args(["query", "findNotes", "not-json"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid JSON"));
}

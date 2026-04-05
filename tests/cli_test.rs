use assert_cmd::Command;
use predicates::prelude::*;

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

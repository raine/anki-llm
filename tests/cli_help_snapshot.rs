use std::{fs, path::PathBuf};

use anki_llm::cli::Cli;
use clap::{CommandFactory, error::ErrorKind};

const COMMANDS: &[(&str, &str)] = &[
    ("export", "Export"),
    ("import", "Import"),
    ("process-file", "ProcessFile"),
    ("process-deck", "ProcessDeck"),
    ("query", "Query"),
    ("config", "Config"),
    ("generate", "Generate"),
    ("generate-init", "GenerateInit"),
    ("history", "History"),
    ("rollback", "Rollback"),
    ("tts", "Tts"),
    ("tts-voices", "TtsVoices"),
    ("update", "Update"),
    ("docs", "Docs"),
    ("workspace", "Workspace"),
    ("note-type", "NoteType"),
    ("doctor", "Doctor"),
];

#[test]
fn root_help_matches_snapshot() {
    assert_snapshot("root.txt", &help_for(&["--help"]));
}

#[test]
fn top_level_subcommand_help_matches_snapshots() {
    for (name, _) in COMMANDS {
        assert_snapshot(&format!("{name}.txt"), &help_for(&[name, "--help"]));
    }
}

#[test]
fn top_level_command_manifest_matches_snapshot() {
    let mut command = Cli::command();
    command.build();

    let actual_commands = command
        .get_subcommands()
        .map(|subcommand| subcommand.get_name())
        .filter(|name| *name != "help")
        .collect::<Vec<_>>();
    let expected_commands = COMMANDS.iter().map(|(name, _)| *name).collect::<Vec<_>>();
    assert_eq!(actual_commands, expected_commands);

    let app_rs = fs::read_to_string(manifest_dir().join("src/app.rs")).unwrap();
    let mut manifest = String::new();
    for (command_name, variant) in COMMANDS {
        manifest.push_str(command_name);
        manifest.push_str(" => Commands::");
        manifest.push_str(variant);
        manifest.push_str(" => ");
        manifest.push_str(&dispatch_target(&app_rs, variant));
        manifest.push('\n');
    }

    assert_snapshot("command-manifest.txt", &manifest);
}

fn help_for(args: &[&str]) -> String {
    let command = Cli::command().color(clap::ColorChoice::Never);
    let error = command
        .try_get_matches_from(std::iter::once("anki-llm").chain(args.iter().copied()))
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::DisplayHelp);
    normalize(error.render().to_string())
}

fn dispatch_target(app_rs: &str, variant: &str) -> String {
    let marker = format!("Commands::{variant}");
    let start = app_rs
        .find(&marker)
        .unwrap_or_else(|| panic!("missing dispatch arm for {marker}"));
    let arm = &app_rs[start..];
    let (_, target) = arm
        .split_once("=>")
        .unwrap_or_else(|| panic!("missing dispatch target for {marker}"));
    target
        .lines()
        .next()
        .unwrap()
        .trim()
        .trim_end_matches(',')
        .to_string()
}

fn assert_snapshot(name: &str, actual: &str) {
    let path = snapshot_path(name);
    if std::env::var_os("UPDATE_CLI_HELP_SNAPSHOTS").is_some() {
        fs::write(&path, actual)
            .unwrap_or_else(|err| panic!("failed to write snapshot {}: {err}", path.display()));
        return;
    }

    let expected = fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("failed to read snapshot {}: {err}", path.display()));
    if expected != actual {
        panic!(
            "snapshot mismatch for {}\n{}",
            path.display(),
            first_difference(&expected, actual)
        );
    }
}

fn first_difference(expected: &str, actual: &str) -> String {
    let expected_lines = expected.lines().collect::<Vec<_>>();
    let actual_lines = actual.lines().collect::<Vec<_>>();
    let max = expected_lines.len().max(actual_lines.len());
    for index in 0..max {
        match (expected_lines.get(index), actual_lines.get(index)) {
            (Some(expected), Some(actual)) if expected == actual => {}
            (Some(expected), Some(actual)) => {
                return format!(
                    "first differing line {}\nexpected: {expected:?}\nactual:   {actual:?}",
                    index + 1
                );
            }
            (Some(expected), None) => {
                return format!(
                    "actual ended before line {}\nexpected: {expected:?}",
                    index + 1
                );
            }
            (None, Some(actual)) => {
                return format!("actual has extra line {}\nactual: {actual:?}", index + 1);
            }
            (None, None) => unreachable!(),
        }
    }
    "contents differ without a line difference".to_string()
}

fn snapshot_path(name: &str) -> PathBuf {
    manifest_dir().join("tests/snapshots/cli_help").join(name)
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn normalize(text: String) -> String {
    text.replace("\r\n", "\n")
}

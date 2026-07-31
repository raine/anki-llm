use std::{env, fs, path::PathBuf, process};

use anki_llm::cli::Cli;
use clap::{Arg, Command, CommandFactory};

const OUTPUT_PATH: &str = "docs/src/content/docs/command-reference.md";
const REGENERATE_COMMAND: &str = "just command-reference";

fn main() {
    let check = match env::args().skip(1).collect::<Vec<_>>().as_slice() {
        [] => false,
        [arg] if arg == "--check" => true,
        _ => {
            eprintln!("usage: cargo run --example generate-command-reference -- [--check]");
            process::exit(2);
        }
    };

    let output = generate();
    let path = output_path();

    if check {
        let committed = fs::read_to_string(&path).unwrap_or_default();
        if committed != output {
            eprintln!(
                "command reference is stale; run '{REGENERATE_COMMAND}' and commit {OUTPUT_PATH}"
            );
            process::exit(1);
        }
        return;
    }

    fs::write(&path, output)
        .unwrap_or_else(|error| panic!("failed to write {}: {error}", path.display()));
    println!("generated {OUTPUT_PATH}");
}

fn generate() -> String {
    let mut root = Cli::command().color(clap::ColorChoice::Never);
    root.build();

    let mut output = String::from(
        "---\n\
         title: \"Command reference\"\n\
         description: \"Review every anki-llm command, argument, and option.\"\n\
         ---\n\n\
         <!-- Generated from the Clap command tree by `just command-reference`. -->\n\n\
         This reference is generated from the command-line interface definition.\n\n\
         ## Command index\n\n",
    );
    render_index(&root, &mut Vec::new(), 0, &mut output);
    output.push('\n');
    render_command(&mut root, &mut Vec::new(), &mut output);
    output.truncate(output.trim_end().len());
    output.push('\n');
    output
}

fn render_index(command: &Command, path: &mut Vec<String>, depth: usize, output: &mut String) {
    for subcommand in visible_subcommands(command) {
        path.push(subcommand.get_name().to_owned());
        let command_name = format!("anki-llm {}", path.join(" "));
        output.push_str(&format!(
            "{}- [`{command_name}`](#{})\n",
            "  ".repeat(depth),
            command_anchor(path)
        ));
        render_index(subcommand, path, depth + 1, output);
        path.pop();
    }
}

fn render_command(command: &mut Command, path: &mut Vec<String>, output: &mut String) {
    let command_name = if path.is_empty() {
        "anki-llm".to_owned()
    } else {
        format!("anki-llm {}", path.join(" "))
    };
    let heading_level = if path.len() <= 1 { 2 } else { path.len() + 1 };
    let anchor = command_anchor(path);

    output.push_str(&format!(
        "<a id=\"{anchor}\"></a>\n\n{} `{command_name}`\n\n",
        "#".repeat(heading_level)
    ));
    if let Some(description) = command.get_long_about().or_else(|| command.get_about()) {
        output.push_str(&normalize_text(&description.to_string()));
        output.push_str("\n\n");
    }

    output.push_str("**Usage**\n\n```text\n");
    output.push_str(command.render_usage().to_string().trim());
    output.push_str("\n```\n\n");

    let arguments = command
        .get_arguments()
        .filter(|argument| argument.is_positional() && !argument.is_hide_set())
        .collect::<Vec<_>>();
    render_arguments("Arguments", &arguments, output);

    let options = command
        .get_arguments()
        .filter(|argument| !argument.is_positional() && !argument.is_hide_set())
        .collect::<Vec<_>>();
    render_arguments("Options", &options, output);

    let subcommands = visible_subcommands(command)
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    for mut subcommand in subcommands {
        path.push(subcommand.get_name().to_owned());
        render_command(&mut subcommand, path, output);
        path.pop();
    }
}

fn render_arguments(heading: &str, arguments: &[&Arg], output: &mut String) {
    if arguments.is_empty() {
        return;
    }

    output.push_str(&format!("**{heading}**\n\n"));
    for argument in arguments {
        let description = argument
            .get_long_help()
            .or_else(|| argument.get_help())
            .map(|help| normalize_text(&help.to_string()))
            .unwrap_or_default();
        let label = argument_label(argument);
        output.push_str(&format!("- **`{label}`**"));
        if !description.is_empty() {
            output.push_str(&format!(": {description}"));
        }
        output.push('\n');

        let mut details = Vec::new();
        if argument.is_required_set() {
            details.push("Required: yes".to_owned());
        }
        if !argument.is_hide_default_value_set() && !argument.get_default_values().is_empty() {
            details.push(format!(
                "Default: {}",
                code_values(
                    argument
                        .get_default_values()
                        .iter()
                        .map(|value| value.to_string_lossy())
                )
            ));
        }
        if !argument.is_hide_env_set()
            && let Some(environment) = argument.get_env()
        {
            details.push(format!("Environment: `{}`", environment.to_string_lossy()));
        }
        let possible_values = argument
            .get_possible_values()
            .into_iter()
            .filter(|value| !value.is_hide_set())
            .map(|value| value.get_name().to_owned())
            .collect::<Vec<_>>();
        if !argument.is_hide_possible_values_set() && !possible_values.is_empty() {
            details.push(format!(
                "Possible values: {}",
                code_values(possible_values.iter().map(String::as_str))
            ));
        }
        if !details.is_empty() {
            output.push_str(&format!("  - {}\n", details.join("; ")));
        }
    }
    output.push('\n');
}

fn visible_subcommands(command: &Command) -> Vec<&Command> {
    command
        .get_subcommands()
        .filter(|subcommand| subcommand.get_name() != "help" && !subcommand.is_hide_set())
        .collect()
}

fn command_anchor(path: &[String]) -> String {
    if path.is_empty() {
        "command-anki-llm".to_owned()
    } else {
        format!("command-{}", path.join("-"))
    }
}

fn argument_label(argument: &Arg) -> String {
    let rendered = argument.to_string();
    match (argument.get_short(), argument.get_long()) {
        (Some(short), Some(_)) => format!("-{short}, {rendered}"),
        _ => rendered,
    }
}

fn normalize_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn code_values<I, S>(values: I) -> String
where
    I: Iterator<Item = S>,
    S: AsRef<str>,
{
    values
        .map(|value| format!("`{}`", value.as_ref().replace('`', "\\`")))
        .collect::<Vec<_>>()
        .join(", ")
}

fn output_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(OUTPUT_PATH)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_is_deterministic() {
        assert_eq!(generate(), generate());
    }

    #[test]
    fn output_contains_top_level_and_nested_command_anchors() {
        let output = generate();

        assert!(output.contains("<a id=\"command-export\"></a>"));
        assert!(output.contains("<a id=\"command-config-get\"></a>"));
        assert!(output.contains("<a id=\"command-note-type-push\"></a>"));
        assert!(output.contains("- **`-q, --query <QUERY>`**"));
    }
}

use std::io::{self, Write};

use anyhow::Result;
use serde_json::Value;

use crate::data::Row;
use crate::style::style;

use super::engine::ProcessFn;

/// Run preview mode: process a sample of rows through the LLM, display a
/// diff-like summary of what would change, and prompt the user to confirm
/// before proceeding with the full run.
///
/// Returns `true` if the user confirmed and processing should continue,
/// `false` if the user declined or preview processing failed.
pub fn run_preview(
    rows: &[Row],
    preview_count: usize,
    process_fn: &ProcessFn,
    source_name: &str,
    id_extractor: &dyn Fn(&Row) -> String,
) -> Result<bool> {
    let sample: Vec<&Row> = rows.iter().take(preview_count).collect();
    if sample.is_empty() {
        return Ok(true);
    }

    let s = style();
    let total = rows.len();

    eprintln!();
    eprintln!("{}", s.bold("═".repeat(72)));
    eprintln!("  {}", s.accent("PREVIEW MODE"));
    eprintln!("{}", s.bold("═".repeat(72)));
    eprintln!("  Source:  {source_name}");
    eprintln!(
        "  Sample:  {} of {} card{}",
        sample.len(),
        total,
        if total == 1 { "" } else { "s" }
    );
    eprintln!("{}", s.bold("═".repeat(72)));

    let mut processed = Vec::with_capacity(sample.len());
    for (i, row) in sample.iter().enumerate() {
        eprintln!();
        eprint!("  Processing sample {} of {}... ", i + 1, sample.len());
        io::stderr().flush()?;

        match process_fn(row) {
            Ok((out_row, _)) => {
                eprintln!("{}", s.green("ok"));
                processed.push(((*row).clone(), out_row));
            }
            Err(e) => {
                eprintln!("{}", s.red("failed"));
                eprintln!();
                eprintln!(
                    "  {} Preview failed for card {}: {e}",
                    s.error_text("error:"),
                    id_extractor(row)
                );
                return Ok(false);
            }
        }
    }

    eprintln!();
    eprintln!("{}", s.bold("─".repeat(72)));

    for (i, (original, output)) in processed.iter().enumerate() {
        print_card_preview(i + 1, original, output, id_extractor);
    }

    eprintln!("{}", s.bold("─".repeat(72)));

    prompt_continue()
}

fn print_card_preview(
    num: usize,
    original: &Row,
    output: &Row,
    id_extractor: &dyn Fn(&Row) -> String,
) {
    let s = style();
    let id = id_extractor(original);

    let first_field = original
        .iter()
        .filter(|(k, _)| !k.starts_with('_'))
        .find_map(|(_, v)| v.as_str().filter(|s| !s.is_empty()))
        .unwrap_or("(empty)");

    eprintln!();
    eprintln!(
        "  {}  {} {}",
        s.bold(format!("Card {num}")),
        s.dim("noteId:"),
        s.dim(&id)
    );
    eprintln!(
        "  {} {}",
        s.dim("Preview:"),
        s.dim(first_field.chars().take(58).collect::<String>())
    );
    eprintln!("  {}", s.dim("─".repeat(66)));

    // Collect all non-internal keys from both rows, preserving order where
    // possible.
    let mut all_keys: Vec<String> = original
        .keys()
        .filter(|k| !k.starts_with('_'))
        .cloned()
        .collect();
    for key in output.keys() {
        if !key.starts_with('_') && !all_keys.contains(key) {
            all_keys.push(key.clone());
        }
    }

    let mut any_change = false;

    for key in &all_keys {
        let orig = field_str(original, key);
        let new = field_str(output, key);

        if !original.contains_key(key) {
            any_change = true;
            eprintln!("  {} {}", s.green("+"), s.bold(key));
            for line in wrap_lines(&new, 64) {
                eprintln!("    {} {}", s.green("+"), s.green(line));
            }
        } else if orig != new {
            any_change = true;
            eprintln!("  {} {}", s.yellow("~"), s.bold(key));
            for line in wrap_lines(&orig, 64) {
                eprintln!("    {} {}", s.red("-"), s.red(line));
            }
            for line in wrap_lines(&new, 64) {
                eprintln!("    {} {}", s.green("+"), s.green(line));
            }
        } else {
            eprintln!("  {} {}", s.dim("="), s.dim(key));
            for line in wrap_lines(&orig, 64) {
                eprintln!("    {}", s.dim(line));
            }
        }
    }

    if !any_change {
        eprintln!("  {}", s.dim("(no fields changed)"));
    }
}

fn field_str(row: &Row, key: &str) -> String {
    row.get(key)
        .map(|v| match v {
            Value::String(s) => s.clone(),
            Value::Null => String::new(),
            other => other.to_string(),
        })
        .unwrap_or_default()
}

fn wrap_lines(text: &str, width: usize) -> Vec<String> {
    if text.is_empty() {
        return vec!["(empty)".to_string()];
    }
    let mut lines = Vec::new();
    for raw_line in text.lines() {
        let mut remaining = raw_line;
        while remaining.len() > width {
            let split = find_split(remaining, width);
            lines.push(remaining[..split].to_string());
            remaining = &remaining[split..];
        }
        lines.push(remaining.to_string());
    }
    lines
}

fn find_split(text: &str, max_width: usize) -> usize {
    if text.len() <= max_width {
        return text.len();
    }

    text.char_indices()
        .take_while(|(i, _)| *i <= max_width)
        .last()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(max_width)
}

fn prompt_continue() -> Result<bool> {
    let s = style();
    eprintln!();
    eprint!("{} ", s.bold("Proceed with processing all cards? [y/N]:"));
    io::stderr().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    Ok(input.trim().eq_ignore_ascii_case("y"))
}

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::anki::client::AnkiClient;
use crate::cli::ExportArgs;
use crate::data::io::{atomic_write_file, serialize_rows};
use crate::data::rows::Row;
use crate::data::slug::slugify_deck_name;

pub fn run(args: ExportArgs) -> Result<()> {
    let output_path = resolve_output_path(&args.deck, args.output.as_deref())?;

    eprintln!("{}", "=".repeat(60));
    eprintln!("Exporting deck: {}", args.deck);
    eprintln!("{}", "=".repeat(60));

    let client = AnkiClient::new();

    let query = format!("deck:\"{}\"", args.deck);
    let note_ids = client.find_notes(&query).context("failed to query deck")?;
    eprintln!("\n✓ Found {} notes in '{}'.", note_ids.len(), args.deck);

    if note_ids.is_empty() {
        eprintln!("No notes found to export.");
        return Ok(());
    }

    eprintln!("\nDiscovering model type and fields...");
    let model_names = client.find_model_names_for_deck(&args.deck)?;
    let model_name = match model_names.as_slice() {
        [only] => only.clone(),
        [] => bail!("deck '{}' is empty", args.deck),
        many => bail!(
            "deck '{}' contains multiple note types: {}. \
             Filter by note type or export a more specific deck.",
            args.deck,
            many.join(", ")
        ),
    };
    eprintln!("✓ Model type: {model_name}");

    let field_names = client.model_field_names(&model_name)?;
    eprintln!("✓ Fields: {}", field_names.join(", "));

    eprintln!("\nFetching note details...");
    let notes = client.notes_info(&note_ids)?;
    eprintln!("✓ Retrieved information for {} notes.", notes.len());

    let rows: Vec<Row> = notes
        .iter()
        .map(|note| {
            let mut row = Row::new();
            row.insert("noteId".into(), Value::Number(note.note_id.into()));
            for field_name in &field_names {
                let value = note
                    .fields
                    .get(field_name)
                    .map(|f| f.value.replace('\r', ""))
                    .unwrap_or_default();
                row.insert(field_name.clone(), Value::String(value));
            }
            row
        })
        .collect();

    eprintln!("\nWriting to {}...", output_path.display());
    let content = serialize_rows(&rows, &output_path)?;
    atomic_write_file(&output_path, &content)?;
    eprintln!(
        "✓ Successfully exported {} notes to {}",
        notes.len(),
        output_path.display()
    );

    Ok(())
}

/// Resolve output path with three modes:
/// 1. None → auto-generate from deck name with .yaml
/// 2. Starts with "." → extension only, auto-generate filename
/// 3. Otherwise → use as-is
fn resolve_output_path(deck_name: &str, output: Option<&Path>) -> Result<PathBuf> {
    let default_ext = "yaml";

    match output {
        None => {
            let path = PathBuf::from(format!("{}.{}", slugify_deck_name(deck_name), default_ext));
            eprintln!(
                "\n✓ Output file not specified, automatically using '{}'",
                path.display()
            );
            Ok(path)
        }
        Some(p) => {
            let s = p.to_string_lossy();
            if s.starts_with('.') {
                let ext = s.trim_start_matches('.').to_lowercase();
                if !matches!(ext.as_str(), "csv" | "yaml" | "yml") {
                    bail!(
                        "unsupported file extension: '{}'. Use .csv, .yaml, or .yml",
                        s
                    );
                }
                let path = PathBuf::from(format!("{}.{}", slugify_deck_name(deck_name), ext));
                eprintln!(
                    "\n✓ Automatically generating filename: '{}'",
                    path.display()
                );
                Ok(path)
            } else {
                Ok(p.to_path_buf())
            }
        }
    }
}

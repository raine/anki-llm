use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::anki::client::{AnkiClient, anki_quote};
use crate::cli::ExportArgs;
use crate::data::io::{atomic_write_file, serialize_rows};
use crate::data::rows::Row;
use crate::data::slug::slugify_deck_name;

pub fn run(args: ExportArgs) -> Result<()> {
    // When using --query without a deck name, an output path is required
    // since we can't auto-generate a filename.
    if args.deck.is_none() {
        let needs_deck = match args.output.as_deref() {
            None => true,
            Some(p) => p.to_string_lossy().starts_with('.'),
        };
        if needs_deck {
            bail!(
                "output path is required when using --query (cannot auto-generate filename without a deck name)"
            );
        }
    }

    let output_path = resolve_output_path(args.deck.as_deref(), args.output.as_deref())?;

    let source_label = args
        .deck
        .as_deref()
        .map(|d| format!("deck '{d}'"))
        .unwrap_or_else(|| format!("query '{}'", args.query.as_deref().unwrap()));

    eprintln!("{}", "=".repeat(60));
    eprintln!("Exporting {source_label}");
    eprintln!("{}", "=".repeat(60));

    let client = AnkiClient::new();

    // Build query from either deck name or raw query
    let mut query = if let Some(ref q) = args.query {
        q.clone()
    } else {
        format!("deck:{}", anki_quote(args.deck.as_ref().unwrap()))
    };
    if let Some(ref nt) = args.note_type {
        query.push_str(&format!(" note:{}", anki_quote(nt)));
    }

    let note_ids = client.find_notes(&query).context("failed to query notes")?;
    eprintln!("\n✓ Found {} notes for {source_label}.", note_ids.len());

    if note_ids.is_empty() {
        eprintln!("No notes found to export.");
        return Ok(());
    }

    // Fetch all note details in one call, then derive the model name from results
    eprintln!("\nFetching note details...");
    let notes = client.notes_info(&note_ids)?;
    eprintln!("✓ Retrieved information for {} notes.", notes.len());

    eprintln!("\nDiscovering model type and fields...");
    let model_name = if let Some(ref nt) = args.note_type {
        nt.clone()
    } else {
        let mut model_names: Vec<String> = notes.iter().map(|n| n.model_name.clone()).collect();
        model_names.sort();
        model_names.dedup();
        match model_names.as_slice() {
            [only] => only.clone(),
            [] => bail!("no notes found"),
            many => bail!(
                "results contain multiple note types: {}. \
                 Use --note-type to filter.",
                many.join(", ")
            ),
        }
    };
    eprintln!("✓ Model type: {model_name}");

    let field_names = client.model_field_names(&model_name)?;
    eprintln!("✓ Fields: {}", field_names.join(", "));

    // Convert to rows
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
fn resolve_output_path(deck_name: Option<&str>, output: Option<&Path>) -> Result<PathBuf> {
    let default_ext = "yaml";

    match output {
        None => {
            // Caller must ensure deck_name is Some when output is None
            let deck = deck_name.expect("deck name required when output not specified");
            let path = PathBuf::from(format!("{}.{}", slugify_deck_name(deck), default_ext));
            eprintln!(
                "\n✓ Output file not specified, automatically using '{}'",
                path.display()
            );
            Ok(path)
        }
        Some(p) => {
            let s = p.to_string_lossy();
            if s.starts_with('.') {
                let deck = deck_name.expect("deck name required for auto-generated filename");
                let ext = s.trim_start_matches('.').to_lowercase();
                if !matches!(ext.as_str(), "csv" | "yaml" | "yml") {
                    bail!(
                        "unsupported file extension: '{}'. Use .csv, .yaml, or .yml",
                        s
                    );
                }
                let path = PathBuf::from(format!("{}.{}", slugify_deck_name(deck), ext));
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

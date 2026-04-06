use std::path::Path;

use indexmap::IndexMap;
use serde_json::Value;

use crate::data::csv_io::{parse_csv, serialize_csv};
use crate::data::rows::Row;

use super::cards::ValidatedCard;

fn card_fields_to_row(fields: &IndexMap<String, String>) -> Row {
    fields
        .iter()
        .map(|(k, v)| (k.clone(), Value::String(v.clone())))
        .collect()
}

/// Export cards to a file. Appends if the file already exists (with schema
/// validation). Supports .yaml, .yml, and .csv extensions.
pub fn export_cards(cards: &[ValidatedCard], output_path: &Path) -> Result<(), anyhow::Error> {
    let ext = output_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let data: Vec<IndexMap<String, String>> = cards.iter().map(|c| c.anki_fields.clone()).collect();

    if data.is_empty() {
        anyhow::bail!("No cards to export");
    }

    let new_headers: Vec<&str> = data[0].keys().map(|k| k.as_str()).collect();

    // Load existing data if file exists
    let mut existing: Vec<IndexMap<String, String>> = Vec::new();
    if output_path.exists() {
        let content = std::fs::read_to_string(output_path)?;
        existing = match ext.as_str() {
            "yaml" | "yml" => serde_yaml::from_str(&content)?,
            "csv" => {
                let rows = parse_csv(&content).map_err(|e| anyhow::anyhow!("{e}"))?;
                rows.into_iter()
                    .map(|row| {
                        row.into_iter()
                            .map(|(k, v)| {
                                let s = match v {
                                    Value::String(s) => s,
                                    other => other.to_string(),
                                };
                                (k, s)
                            })
                            .collect()
                    })
                    .collect()
            }
            _ => anyhow::bail!("Unsupported format: .{ext}. Use .csv, .yaml, or .yml"),
        };

        // Validate headers match
        if let Some(first) = existing.first() {
            let existing_headers: Vec<&str> = first.keys().map(|k| k.as_str()).collect();
            let existing_set: std::collections::HashSet<&str> =
                existing_headers.iter().copied().collect();
            let new_set: std::collections::HashSet<&str> = new_headers.iter().copied().collect();
            if existing_set != new_set {
                anyhow::bail!(
                    "Schema mismatch: existing fields [{}], new fields [{}]",
                    existing_headers.join(", "),
                    new_headers.join(", ")
                );
            }
        }

        eprintln!(
            "\nAppending {} card(s) to existing {} ({} existing)...",
            cards.len(),
            output_path.display(),
            existing.len()
        );
    } else {
        eprintln!(
            "\nExporting {} card(s) to {}...",
            cards.len(),
            output_path.display()
        );
    }

    let combined: Vec<IndexMap<String, String>> = existing.into_iter().chain(data).collect();

    let content = match ext.as_str() {
        "yaml" | "yml" => serde_yaml::to_string(&combined)?,
        "csv" => {
            let rows: Vec<Row> = combined.iter().map(card_fields_to_row).collect();
            serialize_csv(&rows).map_err(|e| anyhow::anyhow!("{e}"))?
        }
        _ => anyhow::bail!("Unsupported format: .{ext}. Use .csv, .yaml, or .yml"),
    };

    std::fs::write(output_path, content)?;

    eprintln!(
        "\nSuccessfully {} {}",
        if output_path.exists() {
            "appended to"
        } else {
            "exported cards to"
        },
        output_path.display()
    );
    eprintln!("\nTo import into Anki, run:");
    eprintln!(
        "  anki-llm import \"{}\" --deck \"Your Deck Name\"",
        output_path.display()
    );

    Ok(())
}

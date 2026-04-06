use std::path::Path;

use indexmap::IndexMap;

use super::cards::ValidatedCard;

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
                let mut reader = csv::Reader::from_reader(content.as_bytes());
                let mut rows = Vec::new();
                for result in reader.deserialize() {
                    rows.push(result?);
                }
                rows
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
            let mut wtr = csv::Writer::from_writer(Vec::new());
            for row in &combined {
                wtr.serialize(row)?;
            }
            String::from_utf8(wtr.into_inner()?)?
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

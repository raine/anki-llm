use anyhow::{Context, Result, bail};
use indexmap::IndexMap;

use crate::anki::client::AnkiClient;
use crate::anki::schema::AddNoteParams;
use crate::cli::ImportArgs;
use crate::data::io::parse_data_file;

pub fn run(args: ImportArgs) -> Result<()> {
    eprintln!("{}", "=".repeat(60));
    eprintln!(
        "Importing from {} to deck: {}",
        args.input.display(),
        args.deck
    );

    let client = AnkiClient::new();

    let model_name = match args.note_type {
        Some(ref name) => name.clone(),
        None => {
            eprintln!("\nNote type not specified, inferring from deck...");
            let name = client
                .find_model_name_for_deck(&args.deck)?
                .context(format!(
                    "could not infer note type from deck '{}'. \
                     The deck may be empty or not exist. \
                     Specify the note type explicitly using --note-type.",
                    args.deck
                ))?;
            eprintln!("✓ Inferred note type: {name}");
            name
        }
    };

    eprintln!("Note type: {model_name}");
    eprintln!("{}", "=".repeat(60));

    eprintln!("\nReading and parsing {}...", args.input.display());
    let rows = parse_data_file(&args.input)?;
    eprintln!("✓ Found {} rows in {}.", rows.len(), args.input.display());

    if rows.is_empty() {
        eprintln!("No rows to import. Exiting.");
        return Ok(());
    }

    eprintln!("\nValidating fields against note type '{model_name}'...");
    let model_fields = client.model_field_names(&model_name)?;
    eprintln!("✓ Note type fields: {}", model_fields.join(", "));

    let input_fields: Vec<String> = rows[0].keys().cloned().collect();
    let key_field = match args.key_field {
        Some(ref k) => k.clone(),
        None => {
            eprintln!("\nAuto-detecting key field...");
            if input_fields.iter().any(|f| f == "noteId") {
                eprintln!("✓ Found 'noteId' column. Using as key field.");
                "noteId".to_string()
            } else if let Some(first) = model_fields.first() {
                if input_fields.contains(first) {
                    eprintln!(
                        "✓ Using first field of note type '{}' as key: {}",
                        model_name, first
                    );
                    first.clone()
                } else {
                    bail!(
                        "could not auto-detect key field. 'noteId' column not found, \
                         and the note type's first field ('{}') is not in the input file. \
                         Specify the key field manually with --key-field.",
                        first
                    );
                }
            } else {
                bail!("note type has no fields");
            }
        }
    };

    eprintln!("Key field: {key_field}");
    eprintln!("{}", "=".repeat(60));

    if !rows[0].contains_key(&key_field) {
        bail!(
            "key field \"{}\" not found in input file. Available fields: {}",
            key_field,
            input_fields.join(", ")
        );
    }

    let mut seen_keys = std::collections::HashSet::new();
    for (i, row) in rows.iter().enumerate() {
        let key_value = row.get(&key_field).and_then(|v| match v {
            serde_json::Value::String(s) if !s.is_empty() => Some(s.clone()),
            serde_json::Value::Number(n) => Some(n.to_string()),
            _ => None,
        });
        match key_value {
            None => bail!(
                "row {} has blank or non-scalar key field '{}'",
                i + 1,
                key_field
            ),
            Some(k) if !seen_keys.insert(k.clone()) => {
                bail!(
                    "duplicate key '{}' in row {} for key field '{}'",
                    k,
                    i + 1,
                    key_field
                );
            }
            _ => {}
        }
    }

    let file_fields: Vec<&String> = input_fields.iter().filter(|f| *f != &key_field).collect();
    let invalid_fields: Vec<&&String> = file_fields
        .iter()
        .filter(|f| !model_fields.contains(f))
        .collect();

    if !invalid_fields.is_empty() {
        let names: Vec<&str> = invalid_fields.iter().map(|f| f.as_str()).collect();
        eprintln!(
            "\n⚠️  Warning: The following fields do not exist in the note type and will be ignored:"
        );
        eprintln!("  {}", names.join(", "));
    }

    let valid_fields: Vec<&String> = file_fields
        .iter()
        .filter(|f| model_fields.contains(f))
        .copied()
        .collect();
    eprintln!(
        "✓ Valid fields to import: {}",
        valid_fields
            .iter()
            .map(|f| f.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );

    eprintln!("\nFetching existing notes from deck '{}'...", args.deck);
    let existing_ids = client.find_notes(&format!("deck:\"{}\"", args.deck))?;
    let mut key_to_note_id: IndexMap<String, i64> = IndexMap::new();

    if !existing_ids.is_empty() {
        let notes_info = client.notes_info(&existing_ids)?;
        for note in &notes_info {
            let key_value = if key_field == "noteId" {
                note.note_id.to_string()
            } else {
                note.fields
                    .get(&key_field)
                    .map(|f| f.value.clone())
                    .unwrap_or_default()
            };
            if !key_value.is_empty() {
                key_to_note_id.insert(key_value, note.note_id);
            }
        }
    }
    eprintln!(
        "✓ Found {} existing notes with a '{}' field.",
        key_to_note_id.len(),
        key_field
    );

    eprintln!("\nPartitioning notes for insert or update...");
    let mut notes_to_add: Vec<AddNoteParams> = Vec::new();
    let mut notes_to_update: Vec<(i64, IndexMap<String, String>)> = Vec::new();

    for row in &rows {
        let mut fields = IndexMap::new();
        for field_name in &valid_fields {
            let value = row
                .get(field_name.as_str())
                .map(value_to_string)
                .unwrap_or_default();
            fields.insert((*field_name).clone(), value);
        }

        let key_value = row.get(&key_field).map(value_to_string).unwrap_or_default();

        if let Some(&existing_id) = key_to_note_id.get(&key_value) {
            notes_to_update.push((existing_id, fields));
        } else {
            if key_field != "noteId" {
                fields.insert(key_field.clone(), key_value);
            }
            notes_to_add.push(AddNoteParams {
                deck_name: args.deck.clone(),
                model_name: model_name.clone(),
                fields,
                tags: vec!["anki-llm-import".to_string()],
            });
        }
    }

    eprintln!("✓ Partitioning complete:");
    eprintln!("  - {} new notes to add.", notes_to_add.len());
    eprintln!("  - {} existing notes to update.", notes_to_update.len());

    if !notes_to_add.is_empty() {
        eprintln!("\nAdding {} new notes...", notes_to_add.len());
        let results = client.add_notes(&notes_to_add)?;
        let successes = results.iter().filter(|r| r.is_some()).count();
        let failures = results.len() - successes;
        eprintln!("✓ Add operation complete: {successes} succeeded, {failures} failed.");
        if failures > 0 {
            eprintln!("  - Some notes failed to add. Check Anki for possible reasons.");
        }
    }

    if !notes_to_update.is_empty() {
        eprintln!("\nUpdating {} existing notes...", notes_to_update.len());
        let actions: Vec<serde_json::Value> = notes_to_update
            .iter()
            .map(|(id, fields)| {
                serde_json::json!({
                    "action": "updateNoteFields",
                    "params": {
                        "note": {
                            "id": id,
                            "fields": fields,
                        }
                    }
                })
            })
            .collect();

        let results = client.multi(&actions)?;
        let failures: Vec<_> = results.iter().filter(|r| !r.is_null()).collect();
        if !failures.is_empty() {
            eprintln!("✗ Update operation failed for {} notes.", failures.len());
        } else {
            eprintln!(
                "✓ Update operation complete: {} notes updated successfully.",
                notes_to_update.len()
            );
        }
    }

    eprintln!("\nImport process finished.");
    Ok(())
}

fn value_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

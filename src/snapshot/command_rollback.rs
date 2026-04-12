use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::anki::client::AnkiClient;
use crate::cli::RollbackArgs;
use crate::style::style;

use super::store::{load_snapshot, save_snapshot};

const ROLLBACK_BATCH_SIZE: usize = 50;

pub fn run(args: RollbackArgs) -> Result<()> {
    let s = style();
    let mut snapshot = load_snapshot(&args.run_id)?;

    if snapshot.rolled_back {
        eprintln!(
            "Warning: snapshot {} was previously rolled back. Proceeding anyway.",
            args.run_id
        );
    }

    if snapshot.notes.is_empty() {
        eprintln!(
            "Snapshot {} has no note revisions to rollback.",
            args.run_id
        );
        return Ok(());
    }

    eprintln!(
        "Rolling back run {} ({} notes from {})...",
        snapshot.run_id,
        snapshot.note_count,
        snapshot.source_display()
    );

    let anki = AnkiClient::new();

    // Fetch current state of all affected notes
    let note_ids: Vec<i64> = snapshot.notes.iter().map(|n| n.note_id).collect();
    let current_notes = anki
        .notes_info(&note_ids)
        .context("failed to fetch current note state from Anki")?;

    // Build a map of current note fields for conflict detection
    let current_map: std::collections::HashMap<i64, indexmap::IndexMap<String, String>> =
        current_notes
            .into_iter()
            .map(|n| {
                let fields = n
                    .fields
                    .into_iter()
                    .map(|(k, v)| (k, v.value.replace('\r', "")))
                    .collect();
                (n.note_id, fields)
            })
            .collect();

    let mut updates: Vec<Value> = Vec::new();
    let mut conflicts: Vec<(i64, Vec<String>)> = Vec::new();
    let mut missing: Vec<i64> = Vec::new();

    for rev in &snapshot.notes {
        let Some(current_fields) = current_map.get(&rev.note_id) else {
            missing.push(rev.note_id);
            continue;
        };

        // Check for conflicts: compare current values against after_fields
        let mut conflict_fields = Vec::new();
        for (field, after_value) in &rev.after_fields {
            if let Some(current_value) = current_fields.get(field)
                && current_value != after_value
            {
                conflict_fields.push(field.clone());
            }
        }

        if !conflict_fields.is_empty() && !args.force {
            conflicts.push((rev.note_id, conflict_fields));
            continue;
        }

        // Restore only fields that were changed during the run AND still exist
        // on the note type. Skip removed fields to avoid AnkiConnect errors.
        let mut fields = serde_json::Map::new();
        for changed_key in rev.after_fields.keys() {
            if !current_fields.contains_key(changed_key) {
                continue;
            }
            if let Some(old_val) = rev.before_fields.get(changed_key) {
                fields.insert(changed_key.clone(), Value::String(old_val.clone()));
            }
        }

        if fields.is_empty() {
            continue;
        }

        updates.push(json!({
            "action": "updateNoteFields",
            "params": {
                "note": {
                    "id": rev.note_id,
                    "fields": fields
                }
            }
        }));
    }

    // Report conflicts and missing notes
    if !missing.is_empty() {
        eprintln!(
            "\n{} {} note(s) no longer exist in Anki (skipped)",
            s.muted("⚠"),
            missing.len()
        );
    }

    if !conflicts.is_empty() {
        eprintln!(
            "\n{} {} note(s) were modified after this run (conflict):",
            s.error_text("✗"),
            conflicts.len()
        );
        for (note_id, fields) in &conflicts {
            eprintln!("  note {note_id}: fields [{}]", fields.join(", "));
        }
        if !args.force {
            eprintln!("\nUse --force to rollback these notes anyway.");
        }
    }

    if updates.is_empty() {
        eprintln!("\nNo notes to rollback.");
        return Ok(());
    }

    if args.dry_run {
        eprintln!(
            "\n{} {} note(s) would be restored (dry run)",
            s.muted("→"),
            updates.len()
        );
        return Ok(());
    }

    // Execute rollback in batches
    eprintln!("\nRestoring {} note(s)...", updates.len());
    let mut total_failures = 0;
    for chunk in updates.chunks(ROLLBACK_BATCH_SIZE) {
        let results = anki
            .multi(chunk)
            .context("failed to send rollback updates to Anki")?;
        total_failures += results.iter().filter(|r| !r.is_null()).count();
    }

    if total_failures > 0 {
        bail!(
            "{} of {} rollback operations failed",
            total_failures,
            updates.len()
        );
    }

    // Mark snapshot as rolled back if all notes were handled
    if conflicts.is_empty() && missing.is_empty() || args.force {
        snapshot.rolled_back = true;
        save_snapshot(&snapshot)?;
    }

    eprintln!(
        "\n{} Successfully rolled back {} note(s).",
        s.success("✓"),
        updates.len()
    );

    Ok(())
}

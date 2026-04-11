use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::anki::client::AnkiClient;
use crate::cli::RollbackArgs;
use crate::style::style;

use super::store::{load_snapshot, save_snapshot};

pub fn run(args: RollbackArgs) -> Result<()> {
    let s = style();
    let mut snapshot = load_snapshot(&args.run_id)?;

    if snapshot.rolled_back {
        bail!("snapshot {} has already been rolled back", args.run_id);
    }

    if snapshot.notes.is_empty() {
        eprintln!(
            "Snapshot {} has no note revisions to rollback.",
            args.run_id
        );
        return Ok(());
    }

    eprintln!(
        "Rolling back run {} ({} notes in deck '{}')...",
        snapshot.run_id, snapshot.note_count, snapshot.deck
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
                let fields = n.fields.into_iter().map(|(k, v)| (k, v.value)).collect();
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
                && current_value != after_value {
                    conflict_fields.push(field.clone());
                }
        }

        if !conflict_fields.is_empty() && !args.force {
            conflicts.push((rev.note_id, conflict_fields));
            continue;
        }

        // Build updateNoteFields action restoring before_fields
        let fields: serde_json::Map<String, Value> = rev
            .before_fields
            .iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect();

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

    // Execute rollback
    eprintln!("\nRestoring {} note(s)...", updates.len());
    let results = anki
        .multi(&updates)
        .context("failed to send rollback updates to Anki")?;

    let failures: Vec<_> = results
        .iter()
        .enumerate()
        .filter(|(_, r)| !r.is_null())
        .collect();

    if !failures.is_empty() {
        bail!(
            "{} of {} rollback operations failed",
            failures.len(),
            updates.len()
        );
    }

    // Mark snapshot as rolled back
    snapshot.rolled_back = true;
    save_snapshot(&snapshot)?;

    eprintln!(
        "\n{} Successfully rolled back {} note(s).",
        s.success("✓"),
        updates.len()
    );

    Ok(())
}

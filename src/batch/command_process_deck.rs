use std::collections::HashSet;
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use anyhow::{Context, Result, bail};
use indexmap::IndexMap;
use serde_json::Value;

use crate::anki::client::{AnkiClient, anki_quote};
use crate::cli::ProcessDeckArgs;
use crate::data::slug::slugify_deck_name;
use crate::llm::client::LlmClient;
use crate::llm::logger::LlmLogger;
use crate::llm::runtime::{RuntimeConfigArgs, build_runtime_config};
use crate::snapshot::store::{self, Snapshot};
use crate::template::fill_template;

use super::deck_mode::{ANKI_NOTE_ID_KEY, DeckWriter};
use super::engine::{BatchConfig, run_batch};
use super::process_row::{ProcessRowConfig, build_process_fn};
use super::report::RowOutcome;

pub fn run(args: ProcessDeckArgs) -> Result<()> {
    let anki = AnkiClient::new();

    // Build query from either deck name or raw query
    eprintln!("Fetching notes...");
    let mut query = if let Some(ref q) = args.query {
        q.clone()
    } else {
        format!("deck:{}", anki_quote(args.deck.as_ref().unwrap()))
    };
    if let Some(ref nt) = args.note_type {
        query.push_str(&format!(" note:{}", anki_quote(nt)));
    }

    let source_label = args
        .deck
        .as_deref()
        .map(|d| format!("deck '{d}'"))
        .unwrap_or_else(|| format!("query '{query}'"));

    let mut note_ids = anki
        .find_notes(&query)
        .context("failed to query Anki for notes")?;

    if note_ids.is_empty() {
        eprintln!("No notes found for {source_label}.");
        return Ok(());
    }
    eprintln!("Found {} notes", note_ids.len());

    // Apply limit before fetching details to avoid loading the entire deck
    if let Some(limit) = args.limit
        && note_ids.len() > limit
    {
        eprintln!(
            "Limiting to {} of {} notes (--limit={})",
            limit,
            note_ids.len(),
            limit
        );
        note_ids.truncate(limit);
    }

    // Fetch note details
    eprintln!("Loading note details...");
    let notes_info = anki
        .notes_info(&note_ids)
        .context("failed to fetch note details from Anki")?;
    eprintln!("Loaded {} notes", notes_info.len());

    // Validate note types — fail fast if mixed (unless --note-type filtered)
    if args.note_type.is_none() && !notes_info.is_empty() {
        let first_model = &notes_info[0].model_name;
        let mixed: Vec<_> = notes_info
            .iter()
            .filter(|n| &n.model_name != first_model)
            .map(|n| n.model_name.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        if !mixed.is_empty() {
            bail!(
                "deck contains multiple note types: '{}' and {}. \
                 Use --note-type to filter.",
                first_model,
                mixed
                    .iter()
                    .map(|m| format!("'{m}'"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }

    // Convert to Row format. The note ID is stored under ANKI_NOTE_ID_KEY (a
    // private _-prefixed key) so it cannot collide with real Anki field names
    // or be overwritten by a case-insensitive JSON merge.
    let rows: Vec<_> = notes_info
        .into_iter()
        .map(|note| {
            let mut row = indexmap::IndexMap::new();
            row.insert(ANKI_NOTE_ID_KEY.to_string(), Value::from(note.note_id));
            for (field_name, field_data) in note.fields {
                // Anki stores field content with \r\n line endings on Windows;
                // strip \r so templates and comparisons work consistently.
                let value = field_data.value.replace('\r', "");
                row.insert(field_name, Value::String(value));
            }
            row
        })
        .collect();

    // Capture before_fields for snapshot (keyed by note_id)
    let before_fields: IndexMap<i64, IndexMap<String, String>> = rows
        .iter()
        .filter_map(|row| {
            let note_id = row.get(ANKI_NOTE_ID_KEY).and_then(|v| match v {
                Value::Number(n) => n.as_i64(),
                Value::String(s) => s.parse().ok(),
                _ => None,
            })?;
            let fields: IndexMap<String, String> = row
                .iter()
                .filter(|(k, _)| !k.starts_with('_'))
                .map(|(k, v)| {
                    let s = match v {
                        Value::String(s) => s.clone(),
                        Value::Null => String::new(),
                        other => other.to_string(),
                    };
                    (k.clone(), s)
                })
                .collect();
            Some((note_id, fields))
        })
        .collect();

    // Validate no duplicate note IDs
    let mut seen_ids = HashSet::new();
    for (i, row) in rows.iter().enumerate() {
        let id = row
            .get(ANKI_NOTE_ID_KEY)
            .and_then(|v| match v {
                Value::Number(n) => Some(n.to_string()),
                Value::String(s) => Some(s.clone()),
                _ => None,
            })
            .with_context(|| format!("row {} missing note ID", i + 1))?;
        if !seen_ids.insert(id.clone()) {
            bail!("duplicate noteId '{}' at row {}", id, i + 1);
        }
    }

    // Resolve prompt path
    let prompt_path = crate::workspace::resolver::resolve_prompt_path(args.prompt)?;

    // Read prompt template
    let prompt_template = fs::read_to_string(&prompt_path)
        .with_context(|| format!("failed to read prompt file: {}", prompt_path.display()))?;

    // Build runtime config
    let runtime = build_runtime_config(RuntimeConfigArgs {
        model: args.model.as_deref(),
        api_base_url: args.api_base_url.as_deref(),
        api_key: args.api_key.as_deref(),
        batch_size: Some(args.batch_size),
        max_tokens: args.max_tokens,
        temperature: args.temperature,
        retries: args.retries,
        dry_run: args.dry_run,
    })?;

    eprintln!("\n{}", "=".repeat(60));
    eprintln!("Source: {source_label}");
    eprintln!("Model: {}", runtime.model);
    eprintln!("Batch size: {}", runtime.batch_size);
    eprintln!("Retries: {}", runtime.retries);
    if let Some(t) = runtime.temperature {
        eprintln!("Temperature: {t}");
    }
    if let Some(field) = &args.field {
        eprintln!("Mode: single field ({field})");
    } else {
        eprintln!("Mode: JSON merge");
    }
    eprintln!("{}", "=".repeat(60));

    eprintln!("\n{} notes to process", rows.len());

    // Dry run
    if args.dry_run {
        eprintln!("\n--- DRY RUN MODE ---");
        eprintln!("\nPrompt template:");
        eprintln!("{prompt_template}");
        if let Some(first) = rows.first() {
            eprintln!("\nSample note:");
            eprintln!(
                "{}",
                serde_json::to_string_pretty(first).unwrap_or_default()
            );
            match fill_template(&prompt_template, first) {
                Ok(filled) => {
                    eprintln!("\nSample prompt:");
                    eprintln!("{filled}");
                }
                Err(e) => eprintln!("\nTemplate error: {e}"),
            }
        }
        return Ok(());
    }

    // Build logger
    let logger = LlmLogger::new(args.log.as_deref(), args.very_verbose)?;
    let logger = Arc::new(logger);

    // Build processing closure (shared with process-file)
    let process_fn = build_process_fn(ProcessRowConfig {
        client: Arc::new(LlmClient::from_config(&runtime)),
        model: runtime.model.clone(),
        template: prompt_template.clone(),
        field: args.field.clone(),
        temperature: runtime.temperature,
        max_tokens: runtime.max_tokens,
        require_result_tag: args.require_result_tag,
        logger: Some(logger),
    });

    // Set up deck writer. Pass the existing AnkiClient (no need for a second one).
    let slug = args
        .deck
        .as_deref()
        .map(slugify_deck_name)
        .unwrap_or_else(|| "query-results".to_string());
    let error_log_path = format!("{slug}-errors.jsonl").into();
    let run_id = store::generate_run_id();
    let writer = Arc::new(DeckWriter::new(
        anki,
        runtime.batch_size as usize,
        error_log_path,
        before_fields,
    )?);

    let writer_cb = Arc::clone(&writer);
    let on_row_done: super::engine::OnRowDone = Box::new(move |outcome| {
        writer_cb.on_row_done(outcome);
        // Signal the engine to abort if an Anki flush has failed — no point
        // burning more LLM tokens when results cannot be saved.
        writer_cb.has_flush_error.load(Ordering::Relaxed)
    });

    // Run batch
    let batch_config = BatchConfig {
        batch_size: runtime.batch_size,
        retries: runtime.retries,
        model: runtime.model.clone(),
        output_path: String::new(),
    };

    let (event_tx, _event_rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_for_ctrlc = Arc::clone(&cancel);
    let _ = ctrlc::set_handler(move || {
        cancel_for_ctrlc.store(true, Ordering::SeqCst);
        eprintln!("Interrupting... waiting for active requests to finish.");
    });

    let (outcomes, _tokens, _interrupted) = run_batch(
        rows,
        process_fn,
        &batch_config,
        Some(on_row_done),
        event_tx,
        cancel,
    );

    // Final flush
    if let Err(e) = writer.flush() {
        eprintln!("Error: failed to flush final Anki updates: {e}");
    }

    let has_flush_error = writer.has_flush_error.load(Ordering::SeqCst);
    let anki_updates = writer.success_count();

    if !has_flush_error {
        eprintln!("\nSuccessfully updated {anki_updates} notes in Anki.");
    }

    // Save snapshot for rollback — even on partial failure, so notes that
    // were successfully flushed before the error can still be rolled back.
    let revisions = writer.take_revisions();
    if !revisions.is_empty() {
        let snapshot = Snapshot {
            run_id: run_id.clone(),
            timestamp: store::generate_timestamp(),
            deck: deck_name.to_string(),
            model: runtime.model.clone(),
            note_count: revisions.len(),
            rolled_back: false,
            notes: revisions,
        };
        match store::save_snapshot(&snapshot) {
            Ok(path) => {
                eprintln!(
                    "Snapshot saved: {} (use `anki-llm rollback {}` to undo)",
                    path.display(),
                    run_id
                );
            }
            Err(e) => {
                eprintln!("Warning: failed to save snapshot: {e}");
            }
        }
    }

    if has_flush_error {
        bail!(
            "failed to update Anki — some processed notes were not saved. \
             Check Anki connectivity and try again."
        );
    }

    let failures = outcomes
        .iter()
        .filter(|o| matches!(o, RowOutcome::Failure { .. }))
        .count();

    if failures > 0 {
        bail!("{failures} notes failed processing. Error log: {slug}-errors.jsonl");
    }

    Ok(())
}

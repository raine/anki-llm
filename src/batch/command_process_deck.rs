use std::collections::HashSet;
use std::fs;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::anki::client::{AnkiClient, anki_quote};
use crate::cli::ProcessDeckArgs;
use crate::data::rows::{Row, require_note_id};
use crate::data::slug::slugify_deck_name;
use crate::llm::client::LlmClient;
use crate::llm::runtime::{RuntimeConfigArgs, build_runtime_config};
use crate::template::fill_template;

use super::deck_mode::DeckWriter;
use super::engine::{BatchConfig, run_batch};
use super::process_row::{ProcessRowConfig, build_process_fn};
use super::report::RowOutcome;

pub fn run(args: ProcessDeckArgs) -> Result<()> {
    let anki = AnkiClient::new();
    let deck_name = &args.deck;

    // Build query — optionally filter by note type
    eprintln!("Fetching notes from deck...");
    let mut query = format!("deck:{}", anki_quote(deck_name));
    if let Some(ref nt) = args.note_type {
        query.push_str(&format!(" note:{}", anki_quote(nt)));
    }

    let mut note_ids = anki
        .find_notes(&query)
        .context("failed to query Anki for notes")?;

    if note_ids.is_empty() {
        eprintln!("No notes found in deck '{deck_name}'.");
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

    // Convert to Row format
    let rows: Vec<Row> = notes_info
        .into_iter()
        .map(|note| {
            let mut row = Row::new();
            row.insert("noteId".into(), Value::from(note.note_id));
            for (field_name, field_data) in note.fields {
                let value = field_data.value.replace('\r', "");
                row.insert(field_name, Value::String(value));
            }
            row
        })
        .collect();

    // Validate no duplicate noteIds
    let mut seen_ids = HashSet::new();
    for (i, row) in rows.iter().enumerate() {
        let id = require_note_id(row).with_context(|| format!("row {} missing note ID", i + 1))?;
        if !seen_ids.insert(id.clone()) {
            bail!("duplicate noteId '{}' at row {}", id, i + 1);
        }
    }

    if args.batch_size == 0 {
        bail!("--batch-size must be at least 1");
    }

    // Read prompt template
    let prompt_template = fs::read_to_string(&args.prompt)
        .with_context(|| format!("failed to read prompt file: {}", args.prompt.display()))?;

    // Build runtime config
    let runtime = build_runtime_config(RuntimeConfigArgs {
        model: args.model.as_deref(),
        batch_size: Some(args.batch_size),
        max_tokens: args.max_tokens,
        temperature: args.temperature,
        retries: args.retries,
        dry_run: args.dry_run,
    })?;

    eprintln!("\n{}", "=".repeat(60));
    eprintln!("Deck: {deck_name}");
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

    // Build processing closure (shared with process-file)
    let process_fn = build_process_fn(ProcessRowConfig {
        client: Arc::new(LlmClient::from_config(&runtime)),
        model: runtime.model.clone(),
        template: prompt_template.clone(),
        field: args.field.clone(),
        temperature: runtime.temperature,
        max_tokens: runtime.max_tokens,
        require_result_tag: args.require_result_tag,
    });

    // Set up deck writer
    let slug = slugify_deck_name(deck_name);
    let error_log_path = format!("{slug}-errors.jsonl").into();
    let writer = Arc::new(DeckWriter::new(
        AnkiClient::new(),
        runtime.batch_size as usize,
        error_log_path,
    ));

    let writer_cb = Arc::clone(&writer);
    let on_row_done: super::engine::OnRowDone = Box::new(move |outcome| {
        writer_cb.on_row_done(outcome);
    });

    // Run batch
    let batch_config = BatchConfig {
        batch_size: runtime.batch_size,
        retries: runtime.retries,
        model: runtime.model.clone(),
    };

    let (outcomes, _tokens, _interrupted) =
        run_batch(rows, process_fn, &batch_config, Some(on_row_done));

    // Final flush
    if let Err(e) = writer.flush() {
        eprintln!("Error: failed to flush final Anki updates: {e}");
    }

    // Check for Anki write failures (distinct from LLM processing failures)
    if writer.has_flush_error.load(Ordering::SeqCst) {
        bail!(
            "failed to update Anki — some processed notes were not saved. \
             Check Anki connectivity and try again."
        );
    }

    let anki_updates = writer.success_count();
    eprintln!("\nSuccessfully updated {anki_updates} notes in Anki.");

    let failures = outcomes
        .iter()
        .filter(|o| matches!(o, RowOutcome::Failure { .. }))
        .count();

    if failures > 0 {
        bail!("{failures} notes failed processing. Error log: {slug}-errors.jsonl");
    }

    Ok(())
}

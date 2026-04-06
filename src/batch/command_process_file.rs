use std::collections::HashSet;
use std::fs;
use std::sync::Arc;

use anyhow::{Context, Result, bail};

use crate::cli::ProcessFileArgs;
use crate::data::io::{load_existing_output, parse_data_file};
use crate::data::rows::{Row, get_note_id, require_note_id};
use crate::llm::client::LlmClient;
use crate::llm::runtime::{RuntimeConfigArgs, build_runtime_config};
use crate::template::fill_template;

use super::engine::{BatchConfig, run_batch};
use super::file_mode::FileWriter;
use super::process_row::{ProcessRowConfig, build_process_fn};
use super::report::RowOutcome;

pub fn run(args: ProcessFileArgs) -> Result<()> {
    // Read input
    let rows = parse_data_file(&args.input)
        .with_context(|| format!("failed to read input file: {}", args.input.display()))?;
    if rows.is_empty() {
        eprintln!("No rows found in input file.");
        return Ok(());
    }
    eprintln!("Loaded {} rows from {}", rows.len(), args.input.display());

    if args.batch_size == 0 {
        bail!("--batch-size must be at least 1");
    }

    // Validate all rows have IDs and check for duplicates
    let mut all_ids = Vec::with_capacity(rows.len());
    let mut seen_ids = HashSet::new();
    for (i, row) in rows.iter().enumerate() {
        let id = require_note_id(row)
            .with_context(|| format!("row {} is missing an ID field (noteId/id/Id)", i + 1))?;
        if !seen_ids.insert(id.clone()) {
            bail!("duplicate ID '{}' in row {}", id, i + 1);
        }
        all_ids.push(id);
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
    eprintln!("Model: {}", runtime.model);
    eprintln!("Batch size: {}", runtime.batch_size);
    eprintln!("Retries: {}", runtime.retries);
    if let Some(t) = runtime.temperature {
        eprintln!("Temperature: {t}");
    }
    if let Some(field) = &args.field {
        eprintln!("Mode: single field ({})", field);
    } else {
        eprintln!("Mode: JSON merge");
    }
    eprintln!("{}", "=".repeat(60));

    // Resume: load existing output
    let existing = if args.force {
        indexmap::IndexMap::new()
    } else {
        let existing = load_existing_output(&args.output).with_context(|| {
            format!("failed to read existing output: {}", args.output.display())
        })?;
        if !existing.is_empty() {
            eprintln!("\nFound {} existing rows in output file", existing.len());
        }
        existing
    };

    // Filter rows to process: skip rows already completed successfully.
    // In --field mode, also re-process rows that are missing the target field.
    let target_field = args.field.as_deref();
    let rows_to_process: Vec<Row> = rows
        .into_iter()
        .filter(|row| {
            let Some(id) = get_note_id(row) else {
                return true; // unreachable after validation above
            };
            match existing.get(&id) {
                Some(existing_row) if existing_row.contains_key("_error") => true,
                Some(existing_row) => target_field.is_some_and(|f| !existing_row.contains_key(f)),
                None => true,
            }
        })
        .collect();

    // Apply limit
    let rows_to_process = if let Some(limit) = args.limit {
        rows_to_process.into_iter().take(limit).collect()
    } else {
        rows_to_process
    };

    if rows_to_process.is_empty() {
        eprintln!("All rows already processed. Use --force to reprocess.");
        return Ok(());
    }

    eprintln!("\n{} rows to process", rows_to_process.len());

    // Dry run
    if args.dry_run {
        eprintln!("\n--- DRY RUN MODE ---");
        eprintln!("\nPrompt template:");
        eprintln!("{prompt_template}");
        if let Some(first) = rows_to_process.first() {
            eprintln!("\nSample row:");
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

    // Build processing closure
    let process_fn = build_process_fn(ProcessRowConfig {
        client: Arc::new(LlmClient::from_config(&runtime)),
        model: runtime.model.clone(),
        template: prompt_template.clone(),
        field: args.field.clone(),
        temperature: runtime.temperature,
        max_tokens: runtime.max_tokens,
        require_result_tag: args.require_result_tag,
    });

    // Set up file writer
    let writer = Arc::new(FileWriter::new(
        args.output.clone(),
        all_ids,
        existing,
        runtime.batch_size as usize,
    ));

    let writer_cb = Arc::clone(&writer);
    let on_row_done: super::engine::OnRowDone = Box::new(move |outcome: &RowOutcome| {
        writer_cb.on_row_done(outcome);
    });

    // Run batch
    let batch_config = BatchConfig {
        batch_size: runtime.batch_size,
        retries: runtime.retries,
        model: runtime.model.clone(),
    };

    let (_outcomes, _tokens, _interrupted) = run_batch(
        rows_to_process,
        process_fn,
        &batch_config,
        Some(on_row_done),
    );

    // Final flush
    writer.flush().context("failed to write final output")?;
    eprintln!("\nOutput written to {}", args.output.display());

    Ok(())
}

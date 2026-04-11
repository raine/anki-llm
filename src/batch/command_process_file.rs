use std::collections::HashSet;
use std::fs;
use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;

use anyhow::{Context, Result, bail};

use crate::cli::ProcessFileArgs;
use crate::data::io::{load_existing_output, parse_data_file};
use crate::data::rows::{Row, get_note_id, require_note_id};
use crate::llm::client::LlmClient;
use crate::llm::logger::LlmLogger;
use crate::llm::runtime::{RuntimeConfig, RuntimeConfigArgs, build_runtime_config};
use crate::template::fill_template;

use super::engine::{BatchConfig, ProcessFn, run_batch};
use super::events::{BatchPlan, OutputMode, RowDescriptor};
use super::file_mode::FileWriter;
use super::plain::run_plain_renderer;
use super::process_row::{ProcessRowConfig, build_process_fn};
use super::report::{ERROR_FIELD, RowOutcome};
use super::tui::BatchTuiResult;

pub fn run(args: ProcessFileArgs) -> Result<()> {
    // Read input
    let rows = parse_data_file(&args.input)
        .with_context(|| format!("failed to read input file: {}", args.input.display()))?;
    if rows.is_empty() {
        eprintln!("No rows found in input file.");
        return Ok(());
    }
    eprintln!("Loaded {} rows from {}", rows.len(), args.input.display());

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

    let input_total = rows.len();

    // Resume: load existing output
    let existing = if args.force {
        indexmap::IndexMap::new()
    } else {
        load_existing_output(&args.output)
            .with_context(|| format!("failed to read existing output: {}", args.output.display()))?
    };
    let resume_skipped = existing.len();

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
                Some(existing_row) if existing_row.contains_key(ERROR_FIELD) => true,
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

    // Build sample prompt for preflight
    let sample_prompt = rows_to_process
        .first()
        .and_then(|row| fill_template(&prompt_template, row).ok());

    // Build plan
    let plan = BatchPlan {
        rows: rows_to_process
            .iter()
            .enumerate()
            .map(|(i, row)| {
                let id = get_note_id(row).unwrap_or_default();
                let preview = row
                    .values()
                    .find_map(|v| v.as_str().filter(|s| !s.is_empty()))
                    .unwrap_or("")
                    .chars()
                    .take(40)
                    .collect();
                RowDescriptor {
                    index: i,
                    id,
                    preview,
                }
            })
            .collect(),
        input_total,
        resume_skipped,
        run_total: rows_to_process.len(),
        model: runtime.model.clone(),
        prompt_path: prompt_path.display().to_string(),
        output_path: args.output.display().to_string(),
        output_mode: if let Some(ref field) = args.field {
            OutputMode::SingleField(field.clone())
        } else {
            OutputMode::JsonMerge
        },
        batch_size: runtime.batch_size,
        retries: runtime.retries,
        sample_prompt,
    };

    // Build logger
    let logger = LlmLogger::new(args.log.as_deref(), args.very_verbose)?;
    let logger = Arc::new(logger);

    // Build processing closure
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

    // Set up file writer
    let writer = Arc::new(FileWriter::new(
        args.output.clone(),
        all_ids,
        existing,
        runtime.batch_size as usize,
    ));

    if std::io::stderr().is_terminal() {
        run_with_tui(plan, rows_to_process, process_fn, &runtime, writer)
    } else {
        run_with_plain(plan, rows_to_process, process_fn, &runtime, writer)
    }
}

fn run_with_plain(
    plan: BatchPlan,
    rows_to_process: Vec<Row>,
    process_fn: ProcessFn,
    runtime: &RuntimeConfig,
    writer: Arc<FileWriter>,
) -> Result<()> {
    let num_to_process = plan.run_total;
    let output_path = plan.output_path.clone();

    // Print config summary (plain mode only — TUI shows it in preflight)
    eprintln!("\n{}", "=".repeat(60));
    eprintln!("Model: {}", runtime.model);
    eprintln!("Batch size: {}", runtime.batch_size);
    eprintln!("Retries: {}", runtime.retries);
    if let Some(t) = runtime.temperature {
        eprintln!("Temperature: {t}");
    }
    match &plan.output_mode {
        OutputMode::SingleField(f) => eprintln!("Mode: single field ({f})"),
        OutputMode::JsonMerge => eprintln!("Mode: JSON merge"),
    }
    eprintln!("{}", "=".repeat(60));
    if plan.resume_skipped > 0 {
        eprintln!(
            "\nFound {} existing rows in output file",
            plan.resume_skipped
        );
    }
    eprintln!("\n{num_to_process} rows to process");

    let batch_config = BatchConfig {
        batch_size: runtime.batch_size,
        retries: runtime.retries,
        model: runtime.model.clone(),
        output_path: plan.output_path.clone(),
    };

    let (event_tx, event_rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_for_ctrlc = Arc::clone(&cancel);
    let _ = ctrlc::set_handler(move || {
        cancel_for_ctrlc.store(true, Ordering::SeqCst);
    });

    let writer_cb = Arc::clone(&writer);
    let on_row_done: super::engine::OnRowDone = Box::new(move |outcome: &RowOutcome| {
        writer_cb.on_row_done(outcome);
        false
    });

    let engine_handle = thread::spawn(move || {
        run_batch(
            rows_to_process,
            process_fn,
            &batch_config,
            Some(on_row_done),
            event_tx,
            cancel,
        )
    });

    run_plain_renderer(event_rx, num_to_process);

    let (outcomes, _tokens, _interrupted) = engine_handle.join().unwrap();

    writer.flush().context("failed to write final output")?;
    eprintln!("\nOutput written to {output_path}");

    let failures = outcomes
        .iter()
        .filter(|o| matches!(o, RowOutcome::Failure { .. }))
        .count();

    if failures > 0 {
        bail!("{failures} row(s) failed processing. See _error fields in output file.");
    }

    Ok(())
}

fn run_with_tui(
    plan: BatchPlan,
    mut pending_rows: Vec<Row>,
    process_fn: ProcessFn,
    runtime: &RuntimeConfig,
    writer: Arc<FileWriter>,
) -> Result<()> {
    let output_path = plan.output_path.clone();

    // The process_fn is shared across retry iterations via Arc
    let process_fn = Arc::new(process_fn);

    loop {
        let batch_config = BatchConfig {
            batch_size: runtime.batch_size,
            retries: runtime.retries,
            model: runtime.model.clone(),
            output_path: plan.output_path.clone(),
        };

        let (event_tx, event_rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let (start_tx, start_rx) = mpsc::sync_channel::<()>(1);

        let cancel_for_engine = Arc::clone(&cancel);
        let writer_cb = Arc::clone(&writer);
        let pf = Arc::clone(&process_fn);
        let rows = pending_rows.clone();

        let engine_handle = thread::spawn(move || {
            // Block until TUI confirms start, or return if cancelled
            if start_rx.recv().is_err() {
                return None;
            }

            let on_row_done: super::engine::OnRowDone = Box::new(move |outcome: &RowOutcome| {
                writer_cb.on_row_done(outcome);
                false
            });

            // Wrap Arc<ProcessFn> into ProcessFn for run_batch
            let process: super::engine::ProcessFn = Box::new(move |row| pf(row));

            Some(run_batch(
                rows,
                process,
                &batch_config,
                Some(on_row_done),
                event_tx,
                cancel_for_engine,
            ))
        });

        let tui_result = super::tui::run_tui(plan.clone(), event_rx, cancel, start_tx)?;

        // Wait for engine thread
        let engine_result = engine_handle.join().unwrap();

        match tui_result {
            BatchTuiResult::Cancelled => {
                if engine_result.is_some() {
                    writer.flush().context("failed to write final output")?;
                    eprintln!("\nPartial output written to {output_path}");
                }
                return Ok(());
            }
            BatchTuiResult::Done => {
                writer.flush().context("failed to write final output")?;

                if let Some((outcomes, _, _)) = engine_result {
                    let failures = outcomes
                        .iter()
                        .filter(|o| matches!(o, RowOutcome::Failure { .. }))
                        .count();
                    if failures > 0 {
                        bail!(
                            "{failures} row(s) failed processing. See _error fields in output file."
                        );
                    }
                }
                return Ok(());
            }
            BatchTuiResult::RetryFailed(failed_rows) => {
                writer.flush().context("failed to write final output")?;
                // Strip _error field from rows before retrying
                pending_rows = failed_rows
                    .into_iter()
                    .map(|mut row| {
                        row.shift_remove(ERROR_FIELD);
                        row
                    })
                    .collect();
                continue;
            }
        }
    }
}

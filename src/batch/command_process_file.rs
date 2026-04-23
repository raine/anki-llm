use std::collections::HashSet;
use std::fs;
use std::sync::Arc;

use anyhow::{Context, Result, bail};

use crate::cli::ProcessFileArgs;
use crate::data::io::{load_existing_output, parse_data_file};
use crate::data::rows::{Row, get_note_id, require_note_id};
use crate::llm::client::LlmClient;
use crate::llm::logger::LlmLogger;
use crate::llm::pricing;
use crate::llm::runtime::{RuntimeConfigArgs, build_runtime_config};
use crate::template::fill_template;

use super::controller::{ControllerRuntime, run_batch_controller};
use super::engine::{EngineRunResult, IdExtractor, OnRowDone, ProcessFn};
use super::events::{BatchPlan, BatchSummary, FailedRowInfo, InfoField, OutputMode, RowDescriptor};
use super::file_mode::FileWriter;
use super::preview;
use super::process_row::{ProcessRowConfig, build_process_fn};
use super::report::{ERROR_FIELD, RowOutcome};
use super::session::{BatchSession, SharedSession};

fn file_row_descriptors(rows: &[Row]) -> Vec<RowDescriptor> {
    rows.iter()
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
        .collect()
}

struct FileSession {
    writer: Arc<FileWriter>,
    process: ProcessFn,
    output_path: String,
    model: String,
}

impl BatchSession for FileSession {
    fn process_fn(&self) -> ProcessFn {
        Arc::clone(&self.process)
    }

    fn on_row_done(&self) -> Option<OnRowDone> {
        let writer = Arc::clone(&self.writer);
        Some(Arc::new(move |outcome| writer.on_row_done(outcome)))
    }

    fn id_extractor(&self) -> IdExtractor {
        Arc::new(|row| get_note_id(row).unwrap_or_default())
    }

    fn row_descriptors(&self, rows: &[Row]) -> Vec<RowDescriptor> {
        file_row_descriptors(rows)
    }

    fn finish_iteration(
        &self,
        result: &EngineRunResult,
        plan_run_total: usize,
    ) -> Result<BatchSummary> {
        self.writer
            .flush()
            .context("failed to write final output")?;

        let succeeded = result
            .outcomes
            .iter()
            .filter(|o| matches!(o, RowOutcome::Success(_)))
            .count();
        let failed_rows: Vec<FailedRowInfo> = result
            .outcomes
            .iter()
            .filter_map(|o| match o {
                RowOutcome::Failure { row, error } => Some(FailedRowInfo {
                    id: get_note_id(row).unwrap_or_default(),
                    error: error.clone(),
                    row_data: row.clone(),
                }),
                _ => None,
            })
            .collect();
        let failed = failed_rows.len();
        let cost = pricing::calculate_cost(&self.model, result.usage.input, result.usage.output);
        let interrupted = result.interrupted || result.abort_reason.is_some();
        let can_retry_failed = failed > 0 && !interrupted && result.abort_reason.is_none();

        Ok(BatchSummary {
            planned_total: plan_run_total,
            processed_total: result.outcomes.len(),
            succeeded,
            failed,
            interrupted,
            input_units: result.usage.input,
            output_units: result.usage.output,
            cost,
            elapsed: result.elapsed,
            model: Some(self.model.clone()),
            metrics_label: "Tokens",
            show_cost: true,
            headline: "Batch complete".into(),
            completion_fields: vec![InfoField {
                label: "Output".into(),
                value: self.output_path.clone(),
            }],
            failed_rows,
            can_retry_failed,
        })
    }
}

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

    let prompt_path = args.prompt;

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
        logger: Some(Arc::clone(&logger)),
    });

    // Preview mode: process a sample and ask for confirmation
    if args.preview {
        let id_extractor = |row: &Row| get_note_id(row).unwrap_or_default();
        let proceed = preview::run_preview(
            &rows_to_process,
            args.preview_count as usize,
            &process_fn,
            &args.input.display().to_string(),
            &id_extractor,
        )?;
        if !proceed {
            eprintln!("Preview cancelled — no changes made.");
            return Ok(());
        }
    }

    // Build sample prompt for preflight
    let sample_prompt = rows_to_process
        .first()
        .and_then(|row| fill_template(&prompt_template, row).ok());

    // Build preflight info fields
    let mut preflight_fields = vec![
        InfoField {
            label: "Input".into(),
            value: format!("{} ({} rows)", args.input.display(), input_total),
        },
        InfoField {
            label: "Output".into(),
            value: args.output.display().to_string(),
        },
    ];
    if resume_skipped > 0 {
        preflight_fields.push(InfoField {
            label: "Resuming".into(),
            value: format!("{resume_skipped} rows from prior output"),
        });
    }

    // Build plan
    let plan = BatchPlan {
        item_name_singular: "row",
        item_name_plural: "rows",
        rows: file_row_descriptors(&rows_to_process),
        run_total: rows_to_process.len(),
        model: Some(runtime.model.clone()),
        prompt_path: Some(prompt_path.display().to_string()),
        output_mode: Some(if let Some(ref field) = args.field {
            OutputMode::SingleField(field.clone())
        } else {
            OutputMode::JsonMerge
        }),
        batch_size: runtime.batch_size,
        retries: runtime.retries,
        sample_prompt,
        metrics_label: "Tokens",
        show_cost: true,
        preflight_fields,
    };

    // Set up file writer
    let writer = Arc::new(FileWriter::new(
        args.output.clone(),
        all_ids,
        existing,
        runtime.batch_size as usize,
    ));

    let session: SharedSession = Arc::new(FileSession {
        writer,
        process: process_fn,
        output_path: args.output.display().to_string(),
        model: runtime.model.clone(),
    });

    let controller_runtime = ControllerRuntime {
        batch_size: runtime.batch_size,
        retries: runtime.retries,
        model: Some(runtime.model.clone()),
    };
    let summary = run_batch_controller(plan, &controller_runtime, rows_to_process, session)?;

    if summary.failed > 0 {
        bail!(
            "{} row(s) failed processing. See _error fields in output file.",
            summary.failed
        );
    }

    Ok(())
}

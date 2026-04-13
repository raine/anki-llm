use std::collections::HashSet;
use std::sync::Arc;

use anyhow::{Context, Result, bail};

use crate::anki::client::AnkiClient;
use crate::batch::controller::{ControllerRuntime, run_batch_controller};
use crate::batch::engine::{EngineRunResult, IdExtractor, OnRowDone, ProcessFn};
use crate::batch::events::{BatchPlan, BatchSummary, FailedRowInfo, InfoField, RowDescriptor};
use crate::batch::file_mode::FileWriter;
use crate::batch::report::ERROR_FIELD;
use crate::batch::report::RowOutcome;
use crate::batch::session::{BatchSession, SharedSession};
use crate::cli::TtsFileArgs;
use crate::data::io::{load_existing_output, parse_data_file};
use crate::data::rows::{Row, get_note_id, require_note_id};

use super::cache::TtsCache;
use super::media::{AnkiMediaStore, field_has_sound_tag};
use super::process_row::{TtsProcessConfig, build_tts_process_fn};
use super::provider::build as build_provider;
use super::runtime::{TtsRuntimeArgs, build_tts_runtime};
use super::template::TemplateSource;

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

struct TtsFileSession {
    writer: Arc<FileWriter>,
    process: ProcessFn,
    output_path: String,
}

impl BatchSession for TtsFileSession {
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
            cost: 0.0,
            elapsed: result.elapsed,
            model: None,
            metrics_label: "Characters",
            show_cost: false,
            headline: "TTS complete".into(),
            completion_fields: vec![InfoField {
                label: "Output".into(),
                value: self.output_path.clone(),
            }],
            failed_rows,
            can_retry_failed,
        })
    }
}

pub fn run(args: TtsFileArgs) -> Result<()> {
    let rows = parse_data_file(&args.input)
        .with_context(|| format!("failed to read input file: {}", args.input.display()))?;
    if rows.is_empty() {
        eprintln!("No rows found in input file.");
        return Ok(());
    }
    eprintln!("Loaded {} rows from {}", rows.len(), args.input.display());

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

    let input_total = rows.len();

    // Resume: load prior output. Rows that already have a sound tag in the
    // target field are skipped unless --force.
    let existing = if args.force {
        indexmap::IndexMap::new()
    } else {
        load_existing_output(&args.output)
            .with_context(|| format!("failed to read existing output: {}", args.output.display()))?
    };
    let resume_skipped = existing.len();

    let target_field = args.field.clone();
    let rows_to_process: Vec<Row> = rows
        .into_iter()
        .filter(|row| {
            if args.force {
                return true;
            }
            let Some(id) = get_note_id(row) else {
                return true;
            };

            // A prior output row only counts as "done" if its target field
            // actually contains a sound tag. Rows that carry an `_error`
            // field are always retried. Plain-text values left over from
            // earlier experimentation should be re-processed (otherwise a
            // populated target column in the input file would skip every
            // row forever).
            if let Some(existing_row) = existing.get(&id) {
                if existing_row.contains_key(ERROR_FIELD) {
                    return true;
                }
                let existing_val = existing_row
                    .get(&target_field)
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if field_has_sound_tag(existing_val) {
                    return false;
                }
            }

            // For rows without prior output, skip if the input row's
            // target field is non-empty — either it already contains
            // audio, or it contains plain text we'd be about to clobber.
            // Users who know what they're doing can pass --force.
            let existing_value = row
                .get(&target_field)
                .and_then(|v| v.as_str())
                .unwrap_or("");
            existing_value.trim().is_empty()
        })
        .collect();

    let rows_to_process = if let Some(limit) = args.limit {
        rows_to_process.into_iter().take(limit).collect()
    } else {
        rows_to_process
    };

    if rows_to_process.is_empty() {
        eprintln!("All rows already have audio. Use --force to regenerate.");
        return Ok(());
    }

    let runtime = build_tts_runtime(TtsRuntimeArgs {
        provider: Some(args.provider.as_str()),
        voice: args.voice.as_deref(),
        tts_model: args.tts_model.as_deref(),
        format: Some(args.format.as_str()),
        speed: args.speed,
        api_key: args.api_key.as_deref(),
        api_base_url: args.api_base_url.as_deref(),
        batch_size: args.batch_size,
        retries: args.retries,
        force: args.force,
        dry_run: args.dry_run,
    })?;

    let source = match (&args.template, &args.text_field) {
        (Some(path), None) => TemplateSource::load_file(path.clone())?,
        (None, Some(field)) => TemplateSource::field(field.clone()),
        _ => bail!("exactly one of --template or --text-field is required"),
    };

    if args.dry_run {
        eprintln!("\n--- DRY RUN MODE ---");
        eprintln!("Provider: {}", runtime.provider);
        eprintln!("Voice:    {}", runtime.voice);
        if let Some(ref m) = runtime.model {
            eprintln!("Model:    {m}");
        }
        eprintln!("Source:   {}", source.display_label());
        if let Some(first) = rows_to_process.first() {
            match source.expand(first) {
                Ok(text) => {
                    let normalized = super::text::normalize(&text);
                    eprintln!("\nSample raw:        {text}");
                    eprintln!("Sample normalized: {normalized}");
                }
                Err(e) => eprintln!("\nTemplate error: {e}"),
            }
        }
        return Ok(());
    }

    let provider = build_provider(
        &runtime.provider,
        runtime.api_key.clone(),
        runtime.api_base_url.clone(),
    )
    .map_err(anyhow::Error::msg)?;

    let cache_dir = TtsCache::default_dir()
        .context("failed to locate cache directory (home dir unavailable)")?;
    let cache = Arc::new(TtsCache::new(cache_dir).context("failed to initialize TTS cache")?);
    let media = Arc::new(AnkiMediaStore::new(AnkiClient::new()));

    let source = Arc::new(source);
    let process_fn = build_tts_process_fn(TtsProcessConfig {
        provider,
        cache,
        media,
        source: Arc::clone(&source),
        target_field: args.field.clone(),
        voice: runtime.voice.clone(),
        model: runtime.model.clone(),
        format: runtime.format,
        speed: runtime.speed,
        api_base_url: runtime.api_base_url.clone(),
    });

    let writer = Arc::new(FileWriter::new(
        args.output.clone(),
        all_ids,
        existing,
        runtime.batch_size as usize,
    ));

    let mut preflight_fields = vec![
        InfoField {
            label: "Input".into(),
            value: format!("{} ({} rows)", args.input.display(), input_total),
        },
        InfoField {
            label: "Output".into(),
            value: args.output.display().to_string(),
        },
        InfoField {
            label: "Field".into(),
            value: args.field.clone(),
        },
        InfoField {
            label: "Voice".into(),
            value: format!("{} ({})", runtime.voice, runtime.provider),
        },
        InfoField {
            label: "Text".into(),
            value: source.display_label(),
        },
    ];
    if resume_skipped > 0 {
        preflight_fields.push(InfoField {
            label: "Resuming".into(),
            value: format!("{resume_skipped} rows from prior output"),
        });
    }

    let plan = BatchPlan {
        item_name_singular: "row",
        item_name_plural: "rows",
        rows: file_row_descriptors(&rows_to_process),
        run_total: rows_to_process.len(),
        model: None,
        prompt_path: None,
        output_mode: None,
        batch_size: runtime.batch_size,
        retries: runtime.retries,
        sample_prompt: None,
        metrics_label: "Characters",
        show_cost: false,
        preflight_fields,
    };

    let session: SharedSession = Arc::new(TtsFileSession {
        writer,
        process: process_fn,
        output_path: args.output.display().to_string(),
    });

    let controller_runtime = ControllerRuntime {
        batch_size: runtime.batch_size,
        retries: runtime.retries,
        model: None,
    };
    let summary = run_batch_controller(plan, &controller_runtime, rows_to_process, session)?;

    if summary.failed > 0 {
        bail!(
            "{} row(s) failed TTS generation. See _error fields in output file.",
            summary.failed
        );
    }

    Ok(())
}

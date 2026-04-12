use std::collections::HashSet;
use std::fs;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use indexmap::IndexMap;
use serde_json::Value;

use crate::anki::client::{AnkiClient, anki_quote};
use crate::cli::ProcessDeckArgs;
use crate::data::Row;
use crate::data::slug::slugify_deck_name;
use crate::llm::client::LlmClient;
use crate::llm::logger::LlmLogger;
use crate::llm::pricing;
use crate::llm::runtime::{RuntimeConfigArgs, build_runtime_config};
use crate::snapshot::store::{self, Snapshot};
use crate::template::fill_template;

use super::controller::run_batch_controller;
use super::deck_mode::{ANKI_NOTE_ID_KEY, DeckWriter};
use super::engine::{EngineRunResult, IdExtractor, OnRowDone, ProcessFn};
use super::events::{BatchPlan, BatchSummary, FailedRowInfo, InfoField, OutputMode, RowDescriptor};
use super::process_row::{ProcessRowConfig, build_process_fn};
use super::report::RowOutcome;
use super::session::{BatchSession, SharedSession};

fn deck_row_id(row: &Row) -> String {
    row.get(ANKI_NOTE_ID_KEY)
        .and_then(|v| match v {
            Value::Number(n) => n.as_i64().map(|n| n.to_string()),
            Value::String(s) => Some(s.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

fn deck_row_preview(row: &Row) -> String {
    row.iter()
        .filter(|(k, _)| !k.starts_with('_'))
        .find_map(|(_, v)| v.as_str().filter(|s| !s.is_empty()))
        .unwrap_or("")
        .chars()
        .take(40)
        .collect()
}

fn deck_row_descriptors(rows: &[Row]) -> Vec<RowDescriptor> {
    rows.iter()
        .enumerate()
        .map(|(i, row)| RowDescriptor {
            index: i,
            id: deck_row_id(row),
            preview: deck_row_preview(row),
        })
        .collect()
}

struct DeckSession {
    writer: Arc<DeckWriter>,
    process: ProcessFn,
    source_name: String,
    model: String,
    slug: String,
    run_id: String,
}

impl BatchSession for DeckSession {
    fn process_fn(&self) -> ProcessFn {
        Arc::clone(&self.process)
    }

    fn on_row_done(&self) -> Option<OnRowDone> {
        let writer = Arc::clone(&self.writer);
        Some(Arc::new(move |outcome| writer.on_row_done(outcome)))
    }

    fn id_extractor(&self) -> IdExtractor {
        Arc::new(deck_row_id)
    }

    fn row_descriptors(&self, rows: &[Row]) -> Vec<RowDescriptor> {
        deck_row_descriptors(rows)
    }

    fn finish_iteration(
        &self,
        result: &EngineRunResult,
        plan_run_total: usize,
    ) -> Result<BatchSummary> {
        // Final flush — short-circuits if a previous flush already failed.
        self.writer.flush()?;

        // Build failed_rows from outcomes
        let failed_rows: Vec<FailedRowInfo> = result
            .outcomes
            .iter()
            .filter_map(|o| match o {
                RowOutcome::Failure { row, error } => Some(FailedRowInfo {
                    id: deck_row_id(row),
                    error: error.clone(),
                    row_data: row.clone(),
                }),
                _ => None,
            })
            .collect();

        // Rewrite the error log with the current iteration's failures so
        // retries can supersede earlier failures cleanly.
        self.writer.rewrite_error_log(&failed_rows)?;

        let succeeded = result
            .outcomes
            .iter()
            .filter(|o| matches!(o, RowOutcome::Success(_)))
            .count();
        let failed = failed_rows.len();
        let updated = self.writer.success_count();
        let cost = pricing::calculate_cost(&self.model, result.tokens.input, result.tokens.output);

        let interrupted = result.interrupted || result.abort_reason.is_some();
        let can_retry_failed = failed > 0 && !interrupted && result.abort_reason.is_none();

        let mut completion_fields = vec![
            InfoField {
                label: "Source".into(),
                value: self.source_name.clone(),
            },
            InfoField {
                label: "Updated".into(),
                value: format!("{updated} notes in Anki"),
            },
        ];
        // Snapshot info isn't shown here — `finish_run` runs after the TUI
        // tears down (so revisions from all retries are aggregated), and its
        // eprintln is the canonical place the user sees the snapshot path.
        if failed > 0 {
            completion_fields.push(InfoField {
                label: "Errors".into(),
                value: format!("{}-errors.jsonl", self.slug),
            });
        }

        Ok(BatchSummary {
            planned_total: plan_run_total,
            processed_total: result.outcomes.len(),
            succeeded,
            failed,
            interrupted,
            input_tokens: result.tokens.input,
            output_tokens: result.tokens.output,
            cost,
            elapsed: result.elapsed,
            model: self.model.clone(),
            headline: format!("Updated {updated} notes in Anki"),
            completion_fields,
            failed_rows,
            can_retry_failed,
        })
    }

    fn finish_run(&self) -> Result<()> {
        let revisions = self.writer.take_revisions();
        if revisions.is_empty() {
            return Ok(());
        }
        let snapshot = Snapshot {
            run_id: self.run_id.clone(),
            timestamp: store::generate_timestamp(),
            deck: self.source_name.clone(),
            model: self.model.clone(),
            note_count: revisions.len(),
            rolled_back: false,
            notes: revisions,
        };
        match store::save_snapshot(&snapshot) {
            Ok(path) => {
                eprintln!(
                    "Snapshot saved: {} (use `anki-llm rollback {}` to undo)",
                    path.display(),
                    self.run_id
                );
            }
            Err(e) => {
                eprintln!("Warning: failed to save snapshot: {e}");
            }
        }
        Ok(())
    }
}

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
                "results contain multiple note types: '{}' and {}. \
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
    let rows: Vec<Row> = notes_info
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

    let source_name = args
        .deck
        .clone()
        .or_else(|| args.query.clone())
        .unwrap_or_default();
    let slug = slugify_deck_name(&source_name);
    let error_log_path = format!("{slug}-errors.jsonl").into();
    let run_id = store::generate_run_id();
    let writer = Arc::new(DeckWriter::new(
        anki,
        runtime.batch_size as usize,
        error_log_path,
        before_fields,
    )?);

    // Build sample prompt for preflight
    let sample_prompt = rows
        .first()
        .and_then(|row| fill_template(&prompt_template, row).ok());

    let plan = BatchPlan {
        item_name_singular: "note",
        item_name_plural: "notes",
        rows: deck_row_descriptors(&rows),
        run_total: rows.len(),
        model: runtime.model.clone(),
        prompt_path: prompt_path.display().to_string(),
        output_mode: if let Some(ref field) = args.field {
            OutputMode::SingleField(field.clone())
        } else {
            OutputMode::JsonMerge
        },
        batch_size: runtime.batch_size,
        retries: runtime.retries,
        sample_prompt,
        preflight_fields: vec![
            InfoField {
                label: "Source".into(),
                value: source_label.clone(),
            },
            InfoField {
                label: "Note type".into(),
                value: args.note_type.clone().unwrap_or_else(|| "any".into()),
            },
            InfoField {
                label: "Destination".into(),
                value: "Anki".into(),
            },
            InfoField {
                label: "Error log".into(),
                value: format!("{slug}-errors.jsonl"),
            },
        ],
    };

    let session: SharedSession = Arc::new(DeckSession {
        writer,
        process: process_fn,
        source_name: source_name.clone(),
        model: runtime.model.clone(),
        slug: slug.clone(),
        run_id: run_id.clone(),
    });

    let summary = run_batch_controller(plan, &runtime, rows, session)?;

    if summary.failed > 0 {
        bail!(
            "{} notes failed processing. Error log: {slug}-errors.jsonl",
            summary.failed
        );
    }

    Ok(())
}

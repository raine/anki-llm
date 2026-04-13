use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use indexmap::IndexMap;
use serde_json::Value;

use crate::anki::client::{AnkiClient, anki_quote};
use crate::batch::controller::{ControllerRuntime, run_batch_controller};
use crate::batch::deck_mode::{ANKI_NOTE_ID_KEY, DeckWriter};
use crate::batch::engine::{EngineRunResult, IdExtractor, OnRowDone, ProcessFn};
use crate::batch::events::{BatchPlan, BatchSummary, FailedRowInfo, InfoField, RowDescriptor};
use crate::batch::report::RowOutcome;
use crate::batch::session::{BatchSession, SharedSession};
use crate::cli::TtsArgs;
use crate::data::Row;
use crate::data::slug::slugify_deck_name;

use super::cache::TtsCache;
use super::media::AnkiMediaStore;
use super::process_row::{TtsProcessConfig, build_tts_process_fn};
use super::provider::build as build_provider;
use super::runtime::{TtsRuntimeArgs, build_tts_runtime};
use super::template::TemplateSource;

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

struct TtsDeckSession {
    writer: Arc<DeckWriter>,
    process: ProcessFn,
    source_name: String,
    slug: String,
}

impl BatchSession for TtsDeckSession {
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
        self.writer.flush()?;

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

        self.writer.rewrite_error_log(&failed_rows)?;

        let succeeded = result
            .outcomes
            .iter()
            .filter(|o| matches!(o, RowOutcome::Success(_)))
            .count();
        let failed = failed_rows.len();
        let updated = self.writer.success_count();
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
            input_units: result.usage.input,
            output_units: result.usage.output,
            cost: 0.0,
            elapsed: result.elapsed,
            model: None,
            metrics_label: "Characters",
            show_cost: false,
            headline: format!("Generated audio for {updated} notes"),
            completion_fields,
            failed_rows,
            can_retry_failed,
        })
    }
}

pub fn run(args: TtsArgs) -> Result<()> {
    let anki = AnkiClient::new();

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

    eprintln!("Loading note details...");
    let notes_info = anki
        .notes_info(&note_ids)
        .context("failed to fetch note details from Anki")?;
    eprintln!("Loaded {} notes", notes_info.len());

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

    // Convert note info to Row, validate target field exists on note type.
    if let Some(first) = notes_info.first()
        && !first.fields.contains_key(&args.field)
    {
        bail!(
            "note type '{}' has no field named '{}'",
            first.model_name,
            args.field
        );
    }

    let rows: Vec<Row> = notes_info
        .into_iter()
        .map(|note| {
            let mut row: Row = IndexMap::new();
            row.insert(ANKI_NOTE_ID_KEY.to_string(), Value::from(note.note_id));
            for (field_name, field_data) in note.fields {
                let value = field_data.value.replace('\r', "");
                row.insert(field_name, Value::String(value));
            }
            row
        })
        .collect();

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

    // Skip rows whose target field is already populated — either with a
    // [sound:...] tag (already has audio) or with plain text we'd be
    // about to clobber on replace. Users who know what they're doing can
    // pass --force to overwrite.
    let total_before_skip = rows.len();
    let rows_to_process: Vec<Row> = if args.force {
        rows
    } else {
        rows.into_iter()
            .filter(|row| {
                let existing = row.get(&args.field).and_then(|v| v.as_str()).unwrap_or("");
                existing.trim().is_empty()
            })
            .collect()
    };
    let skipped = total_before_skip - rows_to_process.len();
    if skipped > 0 {
        eprintln!(
            "Skipping {skipped} notes with non-empty {} field (use --force to regenerate)",
            args.field
        );
    }
    if rows_to_process.is_empty() {
        eprintln!("No notes need audio. Use --force to regenerate.");
        return Ok(());
    }

    // Build TTS runtime from CLI + config + env
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

    // Source: template file xor direct field reference.
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

    let source_name = args
        .deck
        .clone()
        .or_else(|| args.query.clone())
        .unwrap_or_default();
    let slug = slugify_deck_name(&source_name);
    let error_log_path: PathBuf = format!("{slug}-tts-errors.jsonl").into();

    let writer = Arc::new(DeckWriter::new(
        anki,
        runtime.batch_size as usize,
        error_log_path,
        before_fields,
    )?);

    let plan = BatchPlan {
        item_name_singular: "note",
        item_name_plural: "notes",
        rows: deck_row_descriptors(&rows_to_process),
        run_total: rows_to_process.len(),
        model: None,
        prompt_path: None,
        output_mode: None,
        batch_size: runtime.batch_size,
        retries: runtime.retries,
        sample_prompt: None,
        metrics_label: "Characters",
        show_cost: false,
        preflight_fields: vec![
            InfoField {
                label: "Source".into(),
                value: source_label.clone(),
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
        ],
    };

    let session: SharedSession = Arc::new(TtsDeckSession {
        writer,
        process: process_fn,
        source_name: source_name.clone(),
        slug: slug.clone(),
    });

    let controller_runtime = ControllerRuntime {
        batch_size: runtime.batch_size,
        retries: runtime.retries,
        model: None,
    };
    let summary = run_batch_controller(plan, &controller_runtime, rows_to_process, session)?;

    if summary.failed > 0 {
        bail!(
            "{} notes failed TTS generation. Error log: {slug}-tts-errors.jsonl",
            summary.failed
        );
    }

    Ok(())
}

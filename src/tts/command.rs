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
use crate::template::frontmatter::parse_prompt_file;

use super::cache::TtsCache;
use super::media::AnkiMediaStore;
use super::process_row::{TtsProcessConfig, build_tts_process_fn};
use super::provider::build as build_provider;
use super::runtime::{TtsRuntimeArgs, build_tts_runtime};
use super::spec::{CliOverrides, ResolvedProvider, ResolvedTtsSpec, resolve as resolve_tts_spec};
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

/// Dispatcher: branch on whether the user passed `--prompt <file>`.
///
/// Both modes are first-class:
/// - **Prompt mode** (`--prompt <file>`) reads a YAML's `tts:` block.
///   Use this when the deck has a stable design checked into version
///   control alongside the LLM prompt.
/// - **Flag mode** (no `--prompt`) takes voice/target/source directly
///   on the CLI. Use this for one-shot fills against decks you don't
///   maintain or for trying TTS before authoring a YAML.
pub fn run(args: TtsArgs) -> Result<()> {
    if args.prompt.is_some() {
        run_prompt_mode(args)
    } else {
        run_flag_mode(args)
    }
}

fn reject_legacy_flags_in_prompt_mode(args: &TtsArgs) -> Result<()> {
    let mut bad: Vec<&'static str> = Vec::new();
    if args.field.is_some() {
        bad.push("--field");
    }
    if args.template.is_some() {
        bad.push("--template");
    }
    if args.text_field.is_some() {
        bad.push("--text-field");
    }
    if args.provider.is_some() {
        bad.push("--provider");
    }
    if args.voice.is_some() {
        bad.push("--voice");
    }
    if args.tts_model.is_some() {
        bad.push("--tts-model");
    }
    if args.format.is_some() {
        bad.push("--format");
    }
    if args.speed.is_some() {
        bad.push("--speed");
    }
    if args.note_type.is_some() {
        bad.push("--note-type");
    }
    if !bad.is_empty() {
        bail!(
            "these flags are not allowed with --prompt (edit the YAML instead): {}",
            bad.join(", ")
        );
    }
    Ok(())
}

fn run_prompt_mode(args: TtsArgs) -> Result<()> {
    reject_legacy_flags_in_prompt_mode(&args)?;

    let prompt_path = args.prompt.as_ref().expect("checked by dispatcher");
    let content = std::fs::read_to_string(prompt_path)
        .with_context(|| format!("failed to read prompt file {}", prompt_path.display()))?;
    let parsed = parse_prompt_file(&content).map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let tts_spec = parsed
        .frontmatter
        .tts
        .as_ref()
        .context("prompt file has no `tts:` block")?;

    let overrides = CliOverrides {
        api_key: args.api_key.as_deref(),
        api_base_url: args.api_base_url.as_deref(),
        azure_region: args.azure_region.as_deref(),
        aws_access_key_id: args.aws_access_key_id.as_deref(),
        aws_secret_access_key: args.aws_secret_access_key.as_deref(),
        aws_region: args.aws_region.as_deref(),
        batch_size: args.batch_size,
        retries: args.retries,
        force: args.force,
        dry_run: args.dry_run,
    };
    let resolved = resolve_tts_spec(tts_spec, &overrides)?;

    // Query: --query wins; otherwise derive from deck + frontmatter.note_type.
    let query = if let Some(ref q) = args.query {
        q.clone()
    } else {
        let deck = args.deck.as_deref().unwrap_or(&parsed.frontmatter.deck);
        format!(
            "deck:{} note:{}",
            anki_quote(deck),
            anki_quote(&parsed.frontmatter.note_type)
        )
    };
    let source_label = if let Some(ref q) = args.query {
        format!("query '{q}'")
    } else {
        let deck = args.deck.as_deref().unwrap_or(&parsed.frontmatter.deck);
        format!("deck '{deck}'")
    };

    let source_name = args
        .deck
        .clone()
        .or_else(|| args.query.clone())
        .unwrap_or_else(|| parsed.frontmatter.deck.clone());
    let field_map = parsed.frontmatter.field_map.clone();
    let expected_note_type = parsed.frontmatter.note_type.clone();
    run_with_resolved(ResolvedRunInputs {
        args: &args,
        anki: AnkiClient::new(),
        query,
        source_label,
        source_name,
        resolved,
        field_map_for_projection: Some(&field_map),
        expected_note_type: Some(&expected_note_type),
    })
}

fn run_flag_mode(args: TtsArgs) -> Result<()> {
    // Manual enforcement of the old clap ArgGroups, now that they've been
    // dropped so --prompt mode can reuse the same struct.
    if args.deck.is_none() && args.query.is_none() {
        bail!("exactly one of <deck> or --query is required");
    }
    if args.deck.is_some() && args.query.is_some() {
        bail!("<deck> and --query are mutually exclusive");
    }
    match (&args.template, &args.text_field) {
        (Some(_), Some(_)) => bail!("--template and --text-field are mutually exclusive"),
        (None, None) => bail!("exactly one of --template or --text-field is required"),
        _ => {}
    }
    let field = args
        .field
        .clone()
        .context("--field is required (or pass --prompt with a YAML containing a tts: block)")?;

    let runtime = build_tts_runtime(TtsRuntimeArgs {
        provider: Some(args.provider.as_deref().unwrap_or("openai")),
        voice: args.voice.as_deref(),
        tts_model: args.tts_model.as_deref(),
        format: Some(args.format.as_deref().unwrap_or("mp3")),
        speed: args.speed,
        api_key: args.api_key.as_deref(),
        api_base_url: args.api_base_url.as_deref(),
        azure_region: args.azure_region.as_deref(),
        aws_access_key_id: args.aws_access_key_id.as_deref(),
        aws_secret_access_key: args.aws_secret_access_key.as_deref(),
        aws_region: args.aws_region.as_deref(),
        batch_size: args.batch_size,
        retries: args.retries,
        force: args.force,
        dry_run: args.dry_run,
    })?;

    let source = match (&args.template, &args.text_field) {
        (Some(path), None) => TemplateSource::load_file(path.clone())?,
        (None, Some(f)) => TemplateSource::field(f.clone()),
        _ => unreachable!("checked above"),
    };

    // Reshape the flag-mode TtsRuntime into the unified ResolvedTtsSpec so
    // the shared body has a single input type. This is a shim, not a merge
    // — build_tts_runtime is still responsible for flag-mode defaulting
    // (including AppConfig.tts_* fallbacks, which prompt mode skips).
    let resolved = ResolvedTtsSpec {
        provider: runtime.provider,
        voice: runtime.voice,
        model: runtime.model,
        format: runtime.format,
        speed: runtime.speed,
        target: field,
        source,
        batch_size: runtime.batch_size,
        retries: runtime.retries,
        force: runtime.force,
        dry_run: runtime.dry_run,
    };

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
    let source_name = args
        .deck
        .clone()
        .or_else(|| args.query.clone())
        .unwrap_or_default();

    let note_type = args.note_type.clone();
    run_with_resolved(ResolvedRunInputs {
        args: &args,
        anki: AnkiClient::new(),
        query,
        source_label,
        source_name,
        resolved,
        field_map_for_projection: None,
        expected_note_type: note_type.as_deref(),
    })
}

struct ResolvedRunInputs<'a> {
    args: &'a TtsArgs,
    anki: AnkiClient,
    query: String,
    source_label: String,
    source_name: String,
    resolved: ResolvedTtsSpec,
    field_map_for_projection: Option<&'a IndexMap<String, String>>,
    expected_note_type: Option<&'a str>,
}

fn run_with_resolved(inputs: ResolvedRunInputs<'_>) -> Result<()> {
    let ResolvedRunInputs {
        args,
        anki,
        query,
        source_label,
        source_name,
        resolved,
        field_map_for_projection,
        expected_note_type,
    } = inputs;
    eprintln!("Fetching notes...");
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

    // Mixed note type check. In legacy mode we only warn when the user
    // didn't pass --note-type; in prompt mode the frontmatter note_type
    // is always known, so we require every row to match.
    if expected_note_type.is_none() && !notes_info.is_empty() {
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
    } else if let Some(expected) = expected_note_type {
        let wrong: Vec<_> = notes_info
            .iter()
            .filter(|n| n.model_name != expected)
            .map(|n| n.model_name.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        if !wrong.is_empty() {
            bail!(
                "query returned notes of unexpected note types: {}. \
                 Expected '{expected}'.",
                wrong
                    .iter()
                    .map(|m| format!("'{m}'"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }

    // Validate target field exists on the (uniform) note type.
    if let Some(first) = notes_info.first()
        && !first.fields.contains_key(&resolved.target)
    {
        bail!(
            "note type '{}' has no field named '{}'",
            first.model_name,
            resolved.target
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

    // Skip rows whose target field is already populated. Users who know
    // what they're doing can pass --force to overwrite.
    let total_before_skip = rows.len();
    let rows_to_process: Vec<Row> = if resolved.force {
        rows
    } else {
        rows.into_iter()
            .filter(|row| {
                let existing = row
                    .get(&resolved.target)
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                existing.trim().is_empty()
            })
            .collect()
    };
    let skipped = total_before_skip - rows_to_process.len();
    if skipped > 0 {
        eprintln!(
            "Skipping {skipped} notes with non-empty {} field (use --force to regenerate)",
            resolved.target
        );
    }
    if rows_to_process.is_empty() {
        eprintln!("No notes need audio. Use --force to regenerate.");
        return Ok(());
    }

    if resolved.dry_run {
        eprintln!("\n--- DRY RUN MODE ---");
        eprintln!("Provider: {}", resolved.provider.id());
        eprintln!("Voice:    {}", resolved.voice);
        if let Some(ref m) = resolved.model {
            eprintln!("Model:    {m}");
        }
        match &resolved.provider {
            ResolvedProvider::Azure { region, .. } | ResolvedProvider::Amazon { region, .. } => {
                eprintln!("Region:   {region}");
            }
            _ => {}
        }
        eprintln!("Source:   {}", resolved.source.display_label());
        if let Some(first) = rows_to_process.first() {
            let eval = super::process_row::build_eval_row(first, field_map_for_projection);
            match resolved.source.expand(&eval) {
                Ok(text) => {
                    let normalized = super::text::normalize(&text);
                    eprintln!("\nSample raw:        {text}");
                    eprintln!("Sample normalized: {normalized}");
                    match super::ir::parse_furigana(&normalized) {
                        Ok(utterance) => {
                            let payload = match resolved.provider.id() {
                                "azure" => super::render::render_ssml(&utterance, &resolved.voice),
                                _ => super::render::render_plain_text(&utterance),
                            };
                            eprintln!("Sample payload:    {payload}");
                        }
                        Err(e) => eprintln!("Parser error: {e}"),
                    }
                }
                Err(e) => eprintln!("\nTemplate error: {e}"),
            }
        }
        return Ok(());
    }

    let endpoint_identity = resolved.provider.endpoint_identity();
    let provider = build_provider(resolved.provider.clone().into_selection());

    let cache_dir = TtsCache::default_dir()
        .context("failed to locate cache directory (home dir unavailable)")?;
    let cache = Arc::new(TtsCache::new(cache_dir).context("failed to initialize TTS cache")?);
    let media = Arc::new(AnkiMediaStore::new(AnkiClient::new()));

    let source = Arc::new(resolved.source.clone());
    let process_fn = build_tts_process_fn(TtsProcessConfig {
        provider,
        cache,
        media,
        source: Arc::clone(&source),
        target_field: resolved.target.clone(),
        voice: resolved.voice.clone(),
        model: resolved.model.clone(),
        format: resolved.format,
        speed: resolved.speed,
        endpoint: endpoint_identity,
        field_map: field_map_for_projection.cloned(),
    });

    let slug = slugify_deck_name(&source_name);
    let error_log_path: PathBuf = format!("{slug}-tts-errors.jsonl").into();

    let writer = Arc::new(DeckWriter::new(
        anki,
        resolved.batch_size as usize,
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
        batch_size: resolved.batch_size,
        retries: resolved.retries,
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
                value: resolved.target.clone(),
            },
            InfoField {
                label: "Voice".into(),
                value: format!("{} ({})", resolved.voice, resolved.provider.id()),
            },
            InfoField {
                label: "Text".into(),
                value: resolved.source.display_label(),
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
        batch_size: resolved.batch_size,
        retries: resolved.retries,
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

use std::collections::HashSet;
use std::sync::mpsc;

use anyhow::Result;

use crate::anki::client::AnkiClient;
use crate::cli::GenerateArgs;
use crate::llm::client::LlmClient;
use crate::llm::logger::LlmLogger;
use crate::llm::parse_json::try_parse_json_array;
use crate::llm::pricing;
use crate::llm::provider::available_models;
use crate::llm::runtime::{RuntimeConfig, RuntimeConfigArgs, build_runtime_config};
use crate::template::frontmatter::{Frontmatter, parse_prompt_file};

use crate::style::style;

use super::anki_import::{import_cards_to_anki, report_import_result};
use super::cards::{ValidatedCard, map_fields_to_anki, validate_cards};
use super::exporter::export_cards;
use super::manual::get_llm_response_manually;
use super::process::run_processors;
use super::processor::{CardCandidate, generate_cards};
use super::sanitize::sanitize_fields;
use super::selector::{display_cards, select_cards_legacy};
use super::tui::{BackendEvent, PipelineStep, SessionInfo, StepStatus, WorkerCommand};
use super::validate::{ValidationResult, validate_anki_assets};

/// Entry point: dispatch to TUI or legacy mode.
pub fn run(args: GenerateArgs) -> Result<()> {
    use std::io::IsTerminal;

    if args.copy || !std::io::stdout().is_terminal() {
        run_legacy(args)
    } else {
        super::tui::run_tui(args)
    }
}

/// Resolve `args.prompt` to a concrete path for non-interactive use (legacy mode, worker thread).
fn require_prompt_path(prompt: &Option<std::path::PathBuf>) -> Result<std::path::PathBuf> {
    crate::workspace::resolver::resolve_prompt_path(prompt.clone())
}

// ---------------------------------------------------------------------------
// Session state — prepared once, reused across terms
// ---------------------------------------------------------------------------

struct PreparedSession {
    frontmatter: Frontmatter,
    prompt_body: String,
    validation: ValidationResult,
    runtime: RuntimeConfig,
    field_map_keys: Vec<String>,
    anki: AnkiClient,
    client: LlmClient,
    logger: LlmLogger,
}

// ---------------------------------------------------------------------------
// TUI worker — runs in a background thread
// ---------------------------------------------------------------------------

/// Pipeline logic for TUI mode. Sets up once, then loops waiting for terms.
pub fn run_pipeline(
    args: GenerateArgs,
    tx: mpsc::Sender<BackendEvent>,
    rx: mpsc::Receiver<WorkerCommand>,
) -> Result<()> {
    macro_rules! log {
        ($($arg:tt)*) => {
            tx.send(BackendEvent::Log(format!($($arg)*))).ok();
        };
    }

    macro_rules! step_start {
        ($step:expr, $detail:expr) => {
            tx.send(BackendEvent::StepUpdate {
                step: $step,
                status: StepStatus::Running($detail),
            })
            .ok();
        };
    }

    macro_rules! step_done {
        ($step:expr, $detail:expr) => {
            tx.send(BackendEvent::StepUpdate {
                step: $step,
                status: StepStatus::Done($detail),
            })
            .ok();
        };
    }

    // --- Session setup (done once) ---

    step_start!(PipelineStep::LoadPrompt, None);
    let prompt_path = require_prompt_path(&args.prompt).map_err(|e| {
        tx.send(BackendEvent::Fatal(format!("{e}"))).ok();
        e
    })?;
    crate::workspace::resolver::save_last_prompt(&prompt_path);
    let content = std::fs::read_to_string(&prompt_path).map_err(|e| {
        tx.send(BackendEvent::Fatal(format!("{e}"))).ok();
        e
    })?;
    let parsed = parse_prompt_file(&content).map_err(|e| {
        tx.send(BackendEvent::Fatal(format!("{e}"))).ok();
        e
    })?;
    let frontmatter = parsed.frontmatter;

    if !parsed.body.contains("{term}") {
        let msg = "Prompt is missing required placeholder: {term}";
        tx.send(BackendEvent::Fatal(msg.to_string())).ok();
        return Err(anyhow::anyhow!("{}", msg));
    }
    if !parsed.body.contains("{count}") {
        let msg = "Prompt is missing required placeholder: {count}";
        tx.send(BackendEvent::Fatal(msg.to_string())).ok();
        return Err(anyhow::anyhow!("{}", msg));
    }

    step_done!(PipelineStep::LoadPrompt, None);
    log!("Loaded prompt for deck: {}", frontmatter.deck);
    log!("Note type: {}", frontmatter.note_type);

    step_start!(PipelineStep::ValidateAnki, None);
    let anki = AnkiClient::new();
    let validation = validate_anki_assets(&anki, &frontmatter).map_err(|e| {
        tx.send(BackendEvent::Fatal(format!("{e}"))).ok();
        e
    })?;
    step_done!(
        PipelineStep::ValidateAnki,
        Some(validation.note_type_fields.join(", "))
    );
    log!(
        "Note type fields: {}",
        validation.note_type_fields.join(", ")
    );

    // Disable very_verbose in TUI mode — raw stderr output would corrupt the display.
    let logger = LlmLogger::new(args.log.as_deref(), false).map_err(|e| {
        tx.send(BackendEvent::Fatal(format!("{e}"))).ok();
        e
    })?;
    let runtime = build_runtime_config(RuntimeConfigArgs {
        model: args.model.as_deref(),
        batch_size: None,
        max_tokens: args.max_tokens,
        temperature: args.temperature,
        retries: args.retries,
        dry_run: false,
    })
    .map_err(|e| {
        tx.send(BackendEvent::Fatal(format!("{e}"))).ok();
        e
    })?;

    let field_map_keys: Vec<String> = frontmatter.field_map.keys().cloned().collect();
    let client = LlmClient::from_config(&runtime);

    let mut session = PreparedSession {
        frontmatter,
        prompt_body: parsed.body,
        validation,
        runtime,
        field_map_keys,
        anki,
        client,
        logger,
    };

    let models: Vec<String> = available_models(args.dry_run)
        .into_iter()
        .map(|s| s.to_string())
        .collect();

    tx.send(BackendEvent::SessionReady(SessionInfo {
        deck: session.frontmatter.deck.clone(),
        note_type: session.frontmatter.note_type.clone(),
        model: session.runtime.model.clone(),
        available_models: models.clone(),
    }))
    .ok();

    // --- Per-term loop ---
    loop {
        match rx.recv() {
            Ok(WorkerCommand::Start(term)) => {
                // Reset step statuses for the new run
                for step in &[PipelineStep::LoadPrompt, PipelineStep::ValidateAnki] {
                    tx.send(BackendEvent::StepUpdate {
                        step: *step,
                        status: StepStatus::Done(None),
                    })
                    .ok();
                }

                match execute_pipeline_for_term(&term, &args, &session, &tx, &rx) {
                    Ok(sent_done) => {
                        // If the pipeline returned early (e.g. Cancel) without
                        // sending RunDone, send one so the TUI can clear its
                        // pending_cancels counter.
                        if !sent_done {
                            tx.send(BackendEvent::RunDone(String::new())).ok();
                        }
                    }
                    Err(e) => {
                        // RunError already sent inside execute_pipeline_for_term
                        log!("Pipeline error: {e}");
                    }
                }
            }
            Ok(WorkerCommand::SetModel(model)) => {
                match build_runtime_config(RuntimeConfigArgs {
                    model: Some(&model),
                    batch_size: None,
                    max_tokens: args.max_tokens,
                    temperature: args.temperature,
                    retries: args.retries,
                    dry_run: false,
                }) {
                    Ok(new_runtime) => {
                        session.client = LlmClient::from_config(&new_runtime);
                        session.runtime = new_runtime;
                        log!("Switched model to {}", session.runtime.model);
                        tx.send(BackendEvent::SessionReady(SessionInfo {
                            deck: session.frontmatter.deck.clone(),
                            note_type: session.frontmatter.note_type.clone(),
                            model: session.runtime.model.clone(),
                            available_models: models.clone(),
                        }))
                        .ok();
                    }
                    Err(e) => {
                        tx.send(BackendEvent::ModelChangeError(format!("{e}"))).ok();
                    }
                }
            }
            Ok(WorkerCommand::Quit) | Err(_) => break,
            _ => {} // Ignore stray commands
        }
    }

    Ok(())
}

/// Returns `Ok(true)` if RunDone was sent, `Ok(false)` if the run ended early
/// (e.g. cancelled) without sending a completion event.
fn execute_pipeline_for_term(
    term: &str,
    args: &GenerateArgs,
    session: &PreparedSession,
    tx: &mpsc::Sender<BackendEvent>,
    rx: &mpsc::Receiver<WorkerCommand>,
) -> Result<bool> {
    macro_rules! log {
        ($($arg:tt)*) => {
            tx.send(BackendEvent::Log(format!($($arg)*))).ok();
        };
    }

    macro_rules! step_start {
        ($step:expr, $detail:expr) => {
            tx.send(BackendEvent::StepUpdate {
                step: $step,
                status: StepStatus::Running($detail),
            })
            .ok();
        };
    }

    macro_rules! step_done {
        ($step:expr, $detail:expr) => {
            tx.send(BackendEvent::StepUpdate {
                step: $step,
                status: StepStatus::Done($detail),
            })
            .ok();
        };
    }

    macro_rules! step_skip {
        ($step:expr) => {
            tx.send(BackendEvent::StepUpdate {
                step: $step,
                status: StepStatus::Skipped,
            })
            .ok();
        };
    }

    macro_rules! step_error {
        ($step:expr, $detail:expr) => {
            tx.send(BackendEvent::StepUpdate {
                step: $step,
                status: StepStatus::Error($detail),
            })
            .ok();
        };
    }

    macro_rules! bail_err {
        ($($arg:tt)*) => {{
            let msg = format!($($arg)*);
            tx.send(BackendEvent::RunError(msg.clone())).ok();
            return Err(anyhow::anyhow!("{}", msg));
        }};
    }

    let tx_log = tx.clone();
    let on_log: &(dyn Fn(&str) + Send + Sync) = &move |msg: &str| {
        tx_log.send(BackendEvent::Log(msg.to_string())).ok();
    };

    let mut generation_cost = 0.0;
    let first_field_name = &session.validation.note_type_fields[0];

    // --- Generate / validate / select loop (supports refresh) ---

    let mut all_validated: Vec<ValidatedCard> = Vec::new();
    let mut seen_keys: HashSet<String> = HashSet::new();
    let mut is_refresh = false;

    let selected_indices = loop {
        // Build exclude list from previously seen first-field values (sorted for determinism)
        let mut exclude_terms: Vec<String> = seen_keys.iter().cloned().collect();
        exclude_terms.sort();

        if is_refresh {
            // Reset sidebar steps so Select doesn't show as running alongside Generate
            step_done!(PipelineStep::Select, None);
        }
        step_start!(PipelineStep::Generate, None);
        if is_refresh {
            log!("Generating {} more card(s) for \"{}\"...", args.count, term,);
        } else {
            log!(
                "Generating {} card(s) for \"{}\" using {}...",
                args.count,
                term,
                session.runtime.model
            );
        }

        let gen_result = generate_cards(
            term,
            &session.prompt_body,
            args.count,
            &session.field_map_keys,
            if exclude_terms.is_empty() {
                None
            } else {
                Some(&exclude_terms)
            },
            &session.client,
            &session.runtime.model,
            session.runtime.temperature,
            session.runtime.max_tokens,
            session.runtime.retries,
            Some(&session.logger),
            on_log,
        );

        let gen_result = match gen_result {
            Ok(r) => r,
            Err(e) => {
                if is_refresh {
                    // Refresh failure: keep existing cards, go back to selection
                    log!("Refresh failed: {e}");
                    tx.send(BackendEvent::AppendCards(Vec::new())).ok();
                    step_error!(PipelineStep::Generate, format!("{e}"));

                    match rx.recv() {
                        Ok(WorkerCommand::Refresh) => {
                            continue;
                        }
                        Ok(WorkerCommand::Selection(indices)) => break indices,
                        Ok(WorkerCommand::Cancel) | Ok(WorkerCommand::Quit) | Err(_) => {
                            return Ok(false);
                        }
                        _ => bail_err!("Unexpected response during selection"),
                    }
                } else {
                    step_error!(PipelineStep::Generate, format!("{e}"));
                    tx.send(BackendEvent::RunError(format!("{e}"))).ok();
                    return Err(e);
                }
            }
        };

        if let Some(ref cost) = gen_result.cost {
            generation_cost += cost.total_cost;
            tx.send(BackendEvent::CostUpdate {
                input_tokens: cost.input_tokens,
                output_tokens: cost.output_tokens,
                cost: cost.total_cost,
            })
            .ok();
            log!(
                "Tokens: {} in / {} out | Cost: {}",
                cost.input_tokens,
                cost.output_tokens,
                pricing::format_cost(cost.total_cost)
            );
        }

        let mut candidates = gen_result.cards;

        if candidates.is_empty() && !is_refresh {
            bail_err!("No cards were generated");
        }

        step_done!(PipelineStep::Generate, None);
        log!("Generated {} card(s)", candidates.len());
        if !candidates.is_empty() && candidates.len() != args.count as usize {
            log!(
                "Warning: requested {} cards, received {}",
                args.count,
                candidates.len()
            );
        }

        // Pre-select processing (transforms + checks before card selection)
        let pre_select_steps = session
            .frontmatter
            .processing
            .as_ref()
            .map(|p| p.pre_select.as_slice())
            .unwrap_or_default();

        let mut pre_select_flags: Vec<super::process::CardFlag> = Vec::new();

        if !pre_select_steps.is_empty() && !candidates.is_empty() {
            step_start!(PipelineStep::PostProcess, None);
            let proc_result = run_processors(
                pre_select_steps,
                candidates,
                &session.field_map_keys,
                &session.client,
                &session.runtime.model,
                session.runtime.temperature,
                session.runtime.max_tokens,
                session.runtime.retries,
                Some(&session.logger),
                on_log,
            )
            .map_err(|e| {
                tx.send(BackendEvent::RunError(format!("{e}"))).ok();
                e
            })?;
            candidates = proc_result.cards;
            pre_select_flags = proc_result.flags;
            generation_cost += proc_result.cost;
            if proc_result.cost > 0.0 {
                tx.send(BackendEvent::CostUpdate {
                    input_tokens: proc_result.input_tokens,
                    output_tokens: proc_result.output_tokens,
                    cost: proc_result.cost,
                })
                .ok();
            }
            if proc_result.rejected_count > 0 {
                log!(
                    "{} card(s) rejected by pre-select checks",
                    proc_result.rejected_count
                );
            }
            step_done!(PipelineStep::PostProcess, None);
        } else {
            step_skip!(PipelineStep::PostProcess);
        }

        // Sanitize and validate
        step_start!(PipelineStep::Validate, None);
        log!("Checking for duplicates...");

        let sanitized_pairs: Vec<_> = candidates
            .into_iter()
            .map(|c| {
                let s = sanitize_fields(&c.fields);
                (c, s)
            })
            .collect();

        let validated = match validate_cards(
            sanitized_pairs,
            &session.frontmatter,
            first_field_name,
            &session.anki,
        ) {
            Ok(v) => v,
            Err(e) => {
                if is_refresh {
                    log!("Validation failed during refresh: {e}");
                    tx.send(BackendEvent::AppendCards(Vec::new())).ok();
                    step_error!(PipelineStep::Validate, format!("{e}"));

                    match rx.recv() {
                        Ok(WorkerCommand::Refresh) => continue,
                        Ok(WorkerCommand::Selection(indices)) => break indices,
                        Ok(WorkerCommand::Cancel) | Ok(WorkerCommand::Quit) | Err(_) => {
                            return Ok(false);
                        }
                        _ => bail_err!("Unexpected response during selection"),
                    }
                } else {
                    step_error!(PipelineStep::Validate, format!("{e}"));
                    tx.send(BackendEvent::RunError(format!("{e}"))).ok();
                    return Err(e);
                }
            }
        };

        // Attach pre-select flags to validated cards (indices match candidates order)
        let mut validated = validated;
        for flag in &pre_select_flags {
            if let Some(card) = validated.get_mut(flag.card_index) {
                card.flags.push(flag.reason.clone());
            }
        }

        // Deduplicate against cards already seen in this session
        let mut new_cards: Vec<ValidatedCard> = Vec::new();
        for card in validated {
            let key = card
                .anki_fields
                .get(first_field_name)
                .map(|s| super::selector::strip_html_tags(s).to_lowercase())
                .unwrap_or_default();
            // Skip dedup for blank keys — let them through
            if key.is_empty() || seen_keys.insert(key) {
                new_cards.push(card);
            }
        }

        let dup_count = new_cards.iter().filter(|c| c.is_duplicate).count();
        step_done!(
            PipelineStep::Validate,
            if dup_count > 0 {
                Some(format!("{dup_count} duplicate(s)"))
            } else {
                None
            }
        );
        if dup_count > 0 {
            log!("Found {dup_count} duplicate(s) (already in Anki)");
        }

        if is_refresh {
            if new_cards.is_empty() {
                log!("No new unique cards generated");
            } else {
                log!("{} new card(s) added", new_cards.len());
            }
            tx.send(BackendEvent::AppendCards(new_cards.clone())).ok();
            all_validated.extend(new_cards);
        } else {
            // Dry-run display
            if args.dry_run {
                step_skip!(PipelineStep::Select);
                step_skip!(PipelineStep::QualityCheck);
                step_skip!(PipelineStep::Finish);
                for (i, card) in new_cards.iter().enumerate() {
                    let dup = if card.is_duplicate {
                        " (Duplicate)"
                    } else {
                        ""
                    };
                    log!("Card {}{dup}", i + 1);
                    for (name, value) in &card.raw_anki_fields {
                        log!("  {name}: {value}");
                    }
                }
                tx.send(BackendEvent::RunDone(
                    "Dry run complete. No cards were imported.".to_string(),
                ))
                .ok();
                return Ok(true);
            }

            if new_cards.is_empty() {
                tx.send(BackendEvent::RunDone(
                    "No cards to select from.".to_string(),
                ))
                .ok();
                return Ok(true);
            }

            all_validated = new_cards;
            step_start!(PipelineStep::Select, None);
            tx.send(BackendEvent::RequestSelection(all_validated.clone()))
                .ok();
        }

        // Wait for user action: Selection, Refresh, or Cancel
        match rx.recv() {
            Ok(WorkerCommand::Refresh) => {
                is_refresh = true;
                continue;
            }
            Ok(WorkerCommand::Selection(indices)) => break indices,
            Ok(WorkerCommand::Cancel) | Ok(WorkerCommand::Quit) | Err(_) => return Ok(false),
            _ => bail_err!("Unexpected response during selection"),
        }
    };

    if selected_indices.is_empty() {
        tx.send(BackendEvent::RunDone("No cards selected.".to_string()))
            .ok();
        return Ok(true);
    }

    let mut selected: Vec<ValidatedCard> = selected_indices
        .iter()
        .filter_map(|&i| all_validated.get(i).cloned())
        .collect();

    step_done!(
        PipelineStep::Select,
        Some(format!("{} card(s) selected", selected.len()))
    );

    // Filter out duplicates
    let dup_selected = selected.iter().filter(|c| c.is_duplicate).count();
    if dup_selected > 0 {
        log!("Skipping {dup_selected} duplicate(s) — already exist in Anki.");
        selected.retain(|c| !c.is_duplicate);
    }

    if selected.is_empty() {
        tx.send(BackendEvent::RunDone(
            "No non-duplicate cards selected.".to_string(),
        ))
        .ok();
        return Ok(true);
    }

    // Post-select processing (transforms + checks after card selection)
    let post_select_steps = session
        .frontmatter
        .processing
        .as_ref()
        .map(|p| p.post_select.as_slice())
        .unwrap_or_default();

    let mut post_select_cost = 0.0;
    let mut post_errors: Vec<String> = Vec::new();

    let mut final_cards: Vec<ValidatedCard> = selected;

    if !post_select_steps.is_empty() {
        step_start!(PipelineStep::QualityCheck, None);

        // Convert ValidatedCards back to CardCandidates for processing
        let candidates: Vec<CardCandidate> = final_cards
            .iter()
            .map(|vc| CardCandidate {
                fields: vc
                    .fields
                    .iter()
                    .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                    .collect(),
            })
            .collect();

        let proc_result = run_processors(
            post_select_steps,
            candidates,
            &session.field_map_keys,
            &session.client,
            &session.runtime.model,
            session.runtime.temperature,
            session.runtime.max_tokens,
            session.runtime.retries,
            Some(&session.logger),
            on_log,
        )
        .map_err(|e| {
            step_error!(PipelineStep::QualityCheck, format!("{e}"));
            tx.send(BackendEvent::RunError(format!("{e}"))).ok();
            e
        })?;

        post_select_cost = proc_result.cost;
        if proc_result.cost > 0.0 {
            tx.send(BackendEvent::CostUpdate {
                input_tokens: proc_result.input_tokens,
                output_tokens: proc_result.output_tokens,
                cost: proc_result.cost,
            })
            .ok();
        }

        // Check if any post-select transform writes to the identity field
        let first_field_key = session.field_map_keys.first().map(|s| s.as_str());
        let needs_revalidation = first_field_key
            .map(|fk| {
                post_select_steps.iter().any(|s| {
                    s.kind == crate::template::frontmatter::ProcessorKind::Transform
                        && s.write_fields().contains(&fk)
                })
            })
            .unwrap_or(false);

        let post_flags = proc_result.flags;
        let post_rejected_count = proc_result.rejected_count;
        post_errors = proc_result.errors;

        final_cards = proc_result
            .cards
            .into_iter()
            .map(|c| {
                let sanitized = sanitize_fields(&c.fields);

                let anki_fields =
                    map_fields_to_anki(&sanitized, &session.frontmatter.field_map).unwrap();
                let raw_strings: std::collections::HashMap<String, String> = c
                    .fields
                    .iter()
                    .map(|(k, v)| {
                        let s = match v {
                            serde_json::Value::String(s) => s.clone(),
                            other => other.to_string(),
                        };
                        (k.clone(), s)
                    })
                    .collect();
                let raw_anki_fields =
                    map_fields_to_anki(&raw_strings, &session.frontmatter.field_map).unwrap();

                ValidatedCard {
                    fields: sanitized,
                    anki_fields,
                    raw_anki_fields,
                    is_duplicate: false,
                    flags: Vec::new(),
                }
            })
            .collect();

        // Re-check duplicates if identity field may have changed
        if needs_revalidation {
            let first_field_name = &session.validation.note_type_fields[0];
            for card in &mut final_cards {
                if let Some(val) = card
                    .anki_fields
                    .get(first_field_name)
                    .filter(|v| !v.is_empty())
                {
                    let escaped = val
                        .replace('\\', "\\\\")
                        .replace('"', "\\\"")
                        .replace('*', "\\*")
                        .replace('_', "\\_");
                    let query = format!(
                        "\"note:{}\" \"deck:{}\" \"{escaped}\"",
                        session.frontmatter.note_type, session.frontmatter.deck
                    );
                    card.is_duplicate = session
                        .anki
                        .find_notes(&query)
                        .map(|ids| !ids.is_empty())
                        .unwrap_or(false);
                }
            }
            final_cards.retain(|c| !c.is_duplicate);
        }

        // Handle flagged cards from post-select checks (trigger review)
        let mut passed = Vec::new();
        let mut flagged: Vec<super::process::FlaggedCard> = Vec::new();

        for (i, card) in final_cards.into_iter().enumerate() {
            let card_flags: Vec<&super::process::CardFlag> =
                post_flags.iter().filter(|f| f.card_index == i).collect();
            if card_flags.is_empty() {
                passed.push(card);
            } else {
                let reason = card_flags
                    .iter()
                    .map(|f| f.reason.as_str())
                    .collect::<Vec<_>>()
                    .join("; ");
                flagged.push(super::process::FlaggedCard { card, reason });
            }
        }

        final_cards = passed;

        if !flagged.is_empty() {
            let flagged_count = flagged.len();
            log!("{flagged_count} card(s) flagged by post-select check. Please review.");

            let flagged_clone = flagged.clone();
            tx.send(BackendEvent::RequestReview(flagged_clone)).ok();

            let decisions = match rx.recv() {
                Ok(WorkerCommand::Review(d)) => d,
                Ok(WorkerCommand::Cancel) | Ok(WorkerCommand::Quit) | Err(_) => return Ok(false),
                _ => bail_err!("Unexpected response during review"),
            };

            for (flagged_card, keep) in flagged.into_iter().zip(decisions.iter()) {
                if *keep {
                    final_cards.push(flagged_card.card);
                }
            }
        }

        if post_rejected_count > 0 {
            log!(
                "{} card(s) rejected by post-select checks",
                post_rejected_count
            );
        }
    } else {
        step_skip!(PipelineStep::QualityCheck);
    }

    if final_cards.is_empty() {
        let mut msg = "No cards remaining after processing.".to_string();
        if !post_errors.is_empty() {
            msg.push_str("\n\nErrors:\n");
            for e in &post_errors {
                msg.push_str(&format!("  • {e}\n"));
            }
        }
        tx.send(BackendEvent::RunDone(msg)).ok();
        return Ok(true);
    }

    let total_cost = generation_cost + post_select_cost;
    if total_cost > 0.0 {
        log!("Total cost: {}", pricing::format_cost(total_cost));
    }

    step_done!(PipelineStep::QualityCheck, Some("done".to_string()));

    // Export or import
    step_start!(PipelineStep::Finish, None);

    if let Some(ref output_path) = args.output {
        export_cards(&final_cards, output_path, on_log).map_err(|e| {
            step_error!(PipelineStep::Finish, format!("{e}"));
            tx.send(BackendEvent::RunError(format!("{e}"))).ok();
            e
        })?;
        step_done!(
            PipelineStep::Finish,
            Some(format!("exported to {}", output_path.display()))
        );
        tx.send(BackendEvent::RunDone(format!(
            "Exported {} card(s) to {}",
            final_cards.len(),
            output_path.display()
        )))
        .ok();
    } else {
        let result =
            import_cards_to_anki(&final_cards, &session.frontmatter, &session.anki, on_log)
                .map_err(|e| {
                    step_error!(PipelineStep::Finish, format!("{e}"));
                    tx.send(BackendEvent::RunError(format!("{e}"))).ok();
                    e
                })?;

        if result.failures > 0 {
            step_done!(
                PipelineStep::Finish,
                Some(format!(
                    "{} added, {} failed",
                    result.successes, result.failures
                ))
            );
            let msg = format!(
                "Import completed with errors: {} added, {} failed.",
                result.successes, result.failures
            );
            tx.send(BackendEvent::RunDone(msg)).ok();
        } else {
            step_done!(
                PipelineStep::Finish,
                Some(format!("{} card(s) added", result.successes))
            );
            tx.send(BackendEvent::RunDone(format!(
                "Successfully added {} new note(s) to \"{}\"",
                result.successes, session.frontmatter.deck
            )))
            .ok();
        }
    }

    Ok(true)
}

// ---------------------------------------------------------------------------
// Legacy mode (--copy or non-TTY)
// ---------------------------------------------------------------------------

pub fn run_legacy(args: GenerateArgs) -> Result<()> {
    let term = args.term.clone().ok_or_else(|| {
        anyhow::anyhow!("The <TERM> argument is required in non-interactive mode")
    })?;

    // 1. Load and parse prompt file
    let prompt_path = require_prompt_path(&args.prompt)?;
    crate::workspace::resolver::save_last_prompt(&prompt_path);
    let content = std::fs::read_to_string(&prompt_path)?;
    let parsed = parse_prompt_file(&content)?;
    let frontmatter = parsed.frontmatter;

    if !parsed.body.contains("{term}") {
        anyhow::bail!("Prompt is missing required placeholder: {{term}}");
    }
    if !parsed.body.contains("{count}") {
        anyhow::bail!("Prompt is missing required placeholder: {{count}}");
    }
    let has_processing = frontmatter
        .processing
        .as_ref()
        .map(|p| !p.pre_select.is_empty() || !p.post_select.is_empty())
        .unwrap_or(false);
    if args.copy && has_processing {
        anyhow::bail!("processing is not supported in --copy mode");
    }

    let s = style();
    eprintln!("  {}  {}", s.muted("Deck     "), s.cyan(&frontmatter.deck));
    eprintln!(
        "  {}  {}",
        s.muted("Note type"),
        s.cyan(&frontmatter.note_type)
    );

    // 2. Validate Anki assets
    let anki = AnkiClient::new();
    let validation = validate_anki_assets(&anki, &frontmatter)?;
    eprintln!(
        "  {}  {}",
        s.muted("Fields   "),
        s.muted(validation.note_type_fields.join(", "))
    );

    // 3. Build logger
    let logger = LlmLogger::new(args.log.as_deref(), args.very_verbose)?;

    // 4. Resolve LLM config (skipped in --copy mode — no API key needed)
    // dry_run: false because generate always calls the LLM when not in --copy
    // mode (dry-run only skips the Anki import step). Passing dry_run: true
    // would replace the API key with "dry-run" and cause HTTP 400.
    let runtime = if !args.copy {
        Some(build_runtime_config(RuntimeConfigArgs {
            model: args.model.as_deref(),
            batch_size: None,
            max_tokens: args.max_tokens,
            temperature: args.temperature,
            retries: args.retries,
            dry_run: false,
        })?)
    } else {
        None
    };

    // 5. Generate cards
    let field_map_keys: Vec<String> = frontmatter.field_map.keys().cloned().collect();
    let mut generation_cost = 0.0;
    let mut candidates: Vec<CardCandidate>;

    let client = runtime.as_ref().map(LlmClient::from_config);

    let on_log: &(dyn Fn(&str) + Send + Sync) = &|msg| eprintln!("{msg}");

    if args.copy {
        // Manual copy-paste mode
        let mut row = crate::data::Row::new();
        row.insert("term".into(), serde_json::Value::String(term.clone()));
        row.insert(
            "count".into(),
            serde_json::Value::String(args.count.to_string()),
        );
        let filled = crate::template::fill_template(&parsed.body, &row)?;
        let raw = get_llm_response_manually(&filled)?;

        let parsed_arr = try_parse_json_array(&raw)
            .ok_or_else(|| anyhow::anyhow!("Response is not a valid JSON array"))?;

        let mut skipped = 0;
        candidates = parsed_arr
            .into_iter()
            .filter_map(|obj| {
                let mut fields = std::collections::HashMap::new();
                let mut missing = false;
                for key in &field_map_keys {
                    match obj.get(key) {
                        Some(val) => {
                            fields.insert(key.clone(), val.clone());
                        }
                        None => {
                            eprintln!(
                                "  {}",
                                s.warning(format!(
                                    "Response is missing field \"{key}\". Skipping card."
                                ))
                            );
                            missing = true;
                        }
                    }
                }
                if missing {
                    skipped += 1;
                    None
                } else {
                    Some(CardCandidate { fields })
                }
            })
            .collect();

        if skipped > 0 {
            eprintln!(
                "  {}",
                s.warning(format!("Skipped {skipped} card(s) due to missing fields."))
            );
        }
        eprintln!("  Parsed {} card(s) from response", candidates.len());
    } else {
        let client = client.as_ref().unwrap();

        let rt = runtime.as_ref().unwrap();
        let spinner = crate::spinner::llm_spinner(format!(
            "Generating {} card(s) for \"{}\" using {}...",
            args.count, term, rt.model
        ));

        let result = generate_cards(
            &term,
            &parsed.body,
            args.count,
            &field_map_keys,
            None,
            client,
            &rt.model,
            rt.temperature,
            rt.max_tokens,
            rt.retries,
            Some(&logger),
            on_log,
        )?;
        spinner.finish_and_clear();

        if let Some(ref cost) = result.cost {
            generation_cost = cost.total_cost;
            eprintln!(
                "  {}  {} in / {} out   {}",
                s.muted("Tokens"),
                cost.input_tokens,
                cost.output_tokens,
                s.muted(pricing::format_cost(cost.total_cost))
            );
        }

        candidates = result.cards;

        if candidates.is_empty() {
            anyhow::bail!("No cards were generated");
        }

        eprintln!("  {} card(s) generated", s.green(candidates.len()));

        if candidates.len() != args.count as usize {
            eprintln!(
                "  {}",
                s.warning(format!(
                    "Requested {} cards, received {}",
                    args.count,
                    candidates.len()
                ))
            );
        }
    }

    // 5b. Pre-select processing
    let pre_select_steps = frontmatter
        .processing
        .as_ref()
        .map(|p| p.pre_select.as_slice())
        .unwrap_or_default();

    if !pre_select_steps.is_empty() && !candidates.is_empty() {
        let client_ref = client.as_ref().unwrap();
        let rt = runtime.as_ref().unwrap();
        let spinner = crate::spinner::llm_spinner("Running pre-select processing...".to_string());
        let proc_result = run_processors(
            pre_select_steps,
            candidates,
            &field_map_keys,
            client_ref,
            &rt.model,
            rt.temperature,
            rt.max_tokens,
            rt.retries,
            Some(&logger),
            on_log,
        )?;
        spinner.finish_and_clear();

        candidates = proc_result.cards;
        if proc_result.cost > 0.0 {
            generation_cost += proc_result.cost;
            eprintln!(
                "  Processing tokens: {} in / {} out | Cost: {}",
                proc_result.input_tokens,
                proc_result.output_tokens,
                pricing::format_cost(proc_result.cost)
            );
        }
    }

    // 5. Sanitize and validate

    let sanitized_pairs: Vec<_> = candidates
        .into_iter()
        .map(|c| {
            let s = sanitize_fields(&c.fields);
            (c, s)
        })
        .collect();

    let first_field_name = &validation.note_type_fields[0];
    let validated = validate_cards(sanitized_pairs, &frontmatter, first_field_name, &anki)?;

    let dup_count = validated.iter().filter(|c| c.is_duplicate).count();
    if dup_count > 0 {
        eprintln!(
            "  {}",
            s.muted(format!("{dup_count} duplicate(s) already in Anki"))
        );
    }

    // 6. Select cards
    if args.dry_run {
        display_cards(&validated);
        return Ok(());
    }

    if validated.is_empty() {
        eprintln!("No cards to select from.");
        return Ok(());
    }

    let selected_indices = select_cards_legacy(&validated)?;
    let mut selected: Vec<ValidatedCard> = selected_indices
        .iter()
        .filter_map(|&i| validated.get(i).cloned())
        .collect();

    if selected.is_empty() {
        eprintln!("\nNo cards selected. Exiting.");
        return Ok(());
    }

    let dup_selected = selected.iter().filter(|c| c.is_duplicate).count();
    if dup_selected > 0 {
        eprintln!(
            "  {}",
            s.muted(format!("Skipping {dup_selected} duplicate(s)"))
        );
        selected.retain(|c| !c.is_duplicate);
    }

    if selected.is_empty() {
        eprintln!("No non-duplicate cards selected. Exiting.");
        return Ok(());
    }

    // 7. Post-select processing
    let post_select_steps = frontmatter
        .processing
        .as_ref()
        .map(|p| p.post_select.as_slice())
        .unwrap_or_default();

    let mut final_cards = selected;
    let mut post_select_cost = 0.0;

    if !post_select_steps.is_empty()
        && let (Some(client_ref), Some(rt)) = (client.as_ref(), runtime.as_ref())
    {
        let spinner = crate::spinner::llm_spinner("Running post-select processing...".to_string());

        // Convert ValidatedCards to CardCandidates for processing
        let post_candidates: Vec<CardCandidate> = final_cards
            .iter()
            .map(|vc| CardCandidate {
                fields: vc
                    .fields
                    .iter()
                    .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                    .collect(),
            })
            .collect();

        let proc_result = run_processors(
            post_select_steps,
            post_candidates,
            &field_map_keys,
            client_ref,
            &rt.model,
            rt.temperature,
            rt.max_tokens,
            rt.retries,
            Some(&logger),
            on_log,
        )?;

        spinner.finish_and_clear();
        post_select_cost = proc_result.cost;

        if !proc_result.errors.is_empty() {
            for e in &proc_result.errors {
                eprintln!("  Error: {e}");
            }
        }

        // Rebuild ValidatedCards
        let post_flags = proc_result.flags;
        final_cards = proc_result
            .cards
            .into_iter()
            .map(|c| {
                let sanitized = sanitize_fields(&c.fields);
                let anki_fields = map_fields_to_anki(&sanitized, &frontmatter.field_map).unwrap();
                let raw_strings: std::collections::HashMap<String, String> = c
                    .fields
                    .iter()
                    .map(|(k, v)| {
                        let s = match v {
                            serde_json::Value::String(s) => s.clone(),
                            other => other.to_string(),
                        };
                        (k.clone(), s)
                    })
                    .collect();
                let raw_anki_fields =
                    map_fields_to_anki(&raw_strings, &frontmatter.field_map).unwrap();

                ValidatedCard {
                    fields: sanitized,
                    anki_fields,
                    raw_anki_fields,
                    is_duplicate: false,
                    flags: Vec::new(),
                }
            })
            .collect();

        // Interactive review of flagged cards
        let mut passed = Vec::new();
        let mut flagged_cards = Vec::new();

        for (i, card) in final_cards.into_iter().enumerate() {
            let card_flags: Vec<_> = post_flags.iter().filter(|f| f.card_index == i).collect();
            if card_flags.is_empty() {
                passed.push(card);
            } else {
                let reason = card_flags
                    .iter()
                    .map(|f| f.reason.as_str())
                    .collect::<Vec<_>>()
                    .join("; ");
                flagged_cards.push((card, reason));
            }
        }

        final_cards = passed;

        if !flagged_cards.is_empty() {
            let flagged_count = flagged_cards.len();
            eprintln!("\n{flagged_count} card(s) were flagged. Please review:");

            for (i, (card, reason)) in flagged_cards.into_iter().enumerate() {
                eprintln!("\n--- Flagged Card {}/{} ---", i + 1, flagged_count);
                for (key, value) in &card.fields {
                    eprintln!("{key}: {value}");
                }
                eprintln!("\nReason: {reason}");

                let keep = inquire::Confirm::new("Keep this card anyway?")
                    .with_default(false)
                    .prompt()
                    .unwrap_or(false);

                if keep {
                    final_cards.push(card);
                }
            }
        }
    }

    if final_cards.is_empty() {
        eprintln!("\nNo cards remaining after processing. Exiting.");
        return Ok(());
    }

    let total_cost = generation_cost + post_select_cost;
    if total_cost > 0.0 {
        eprintln!(
            "\n  {}  {}",
            s.muted("Total cost"),
            s.accent(pricing::format_cost(total_cost))
        );
    }

    // 8. Export or import
    if let Some(ref output_path) = args.output {
        export_cards(&final_cards, output_path, on_log)?;
    } else {
        let result = import_cards_to_anki(&final_cards, &frontmatter, &anki, on_log)?;
        report_import_result(&result, &frontmatter.deck);

        if result.failures > 0 {
            anyhow::bail!(
                "Import failed: {} card(s) could not be added. Check your Anki collection and try again.",
                result.failures
            );
        }
    }

    Ok(())
}

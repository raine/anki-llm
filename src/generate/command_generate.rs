use std::collections::HashSet;
use std::sync::mpsc;

use anyhow::Result;

use crate::anki::client::AnkiClient;
use crate::cli::GenerateArgs;
use crate::llm::client::LlmClient;
use crate::llm::logger::LlmLogger;
use crate::llm::parse_json::try_parse_json_array;
use crate::llm::pricing;
use crate::llm::runtime::{RuntimeConfig, RuntimeConfigArgs, build_runtime_config};
use crate::template::frontmatter::{Frontmatter, parse_prompt_file};

use super::anki_import::{import_cards_to_anki, report_import_result};
use super::cards::{ValidatedCard, validate_cards};
use super::exporter::export_cards;
use super::manual::get_llm_response_manually;
use super::processor::{CardCandidate, generate_cards};
use super::quality::run_quality_checks;
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
    let content = std::fs::read_to_string(&args.prompt).map_err(|e| {
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

    let session = PreparedSession {
        frontmatter,
        prompt_body: parsed.body,
        validation,
        runtime,
        field_map_keys,
        anki,
        client,
        logger,
    };

    tx.send(BackendEvent::SessionReady(SessionInfo {
        deck: session.frontmatter.deck.clone(),
        note_type: session.frontmatter.note_type.clone(),
        model: session.runtime.model.clone(),
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
        // Build exclude list from previously seen first-field values
        let exclude_terms: Vec<String> = seen_keys.iter().cloned().collect();

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
                    step_done!(PipelineStep::Generate, Some("failed".to_string()));

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

        let candidates = gen_result.cards;

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

        let validated = validate_cards(
            sanitized_pairs,
            &session.frontmatter,
            first_field_name,
            &session.anki,
        )
        .map_err(|e| {
            tx.send(BackendEvent::RunError(format!("{e}"))).ok();
            e
        })?;

        // Deduplicate against cards already seen in this session
        let mut new_cards: Vec<ValidatedCard> = Vec::new();
        for card in validated {
            let key = card
                .anki_fields
                .get(first_field_name)
                .map(|s| super::selector::strip_html_tags(s).to_lowercase())
                .unwrap_or_default();
            if seen_keys.insert(key) {
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
            all_validated.extend(new_cards.clone());
            tx.send(BackendEvent::AppendCards(new_cards)).ok();
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

    // Quality check
    step_start!(PipelineStep::QualityCheck, None);
    let qc_result = run_quality_checks(
        selected,
        session.frontmatter.quality_check.as_ref(),
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

    if qc_result.cost > 0.0 {
        tx.send(BackendEvent::CostUpdate {
            input_tokens: qc_result.input_tokens,
            output_tokens: qc_result.output_tokens,
            cost: qc_result.cost,
        })
        .ok();
    }

    // If there are flagged cards, ask the TUI to review them
    let mut final_cards = qc_result.passed;

    if !qc_result.flagged.is_empty() {
        let flagged_count = qc_result.flagged.len();
        log!("{flagged_count} card(s) flagged by quality check. Please review.");

        let flagged_clone = qc_result.flagged.clone();
        tx.send(BackendEvent::RequestReview(flagged_clone)).ok();

        let decisions = match rx.recv() {
            Ok(WorkerCommand::Review(d)) => d,
            Ok(WorkerCommand::Cancel) | Ok(WorkerCommand::Quit) | Err(_) => return Ok(false),
            _ => bail_err!("Unexpected response during review"),
        };

        for (flagged, keep) in qc_result.flagged.into_iter().zip(decisions.iter()) {
            if *keep {
                final_cards.push(flagged.card);
            }
        }
    }

    if final_cards.is_empty() {
        tx.send(BackendEvent::RunDone(
            "No cards remaining after quality check.".to_string(),
        ))
        .ok();
        return Ok(true);
    }

    let total_cost = generation_cost + qc_result.cost;
    if total_cost > 0.0 {
        log!("Total cost: {}", pricing::format_cost(total_cost));
    }

    step_done!(PipelineStep::QualityCheck, Some("done".to_string()));

    // Export or import
    step_start!(PipelineStep::Finish, None);

    if let Some(ref output_path) = args.output {
        export_cards(&final_cards, output_path, on_log).map_err(|e| {
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
    let content = std::fs::read_to_string(&args.prompt)?;
    let parsed = parse_prompt_file(&content)?;
    let frontmatter = parsed.frontmatter;

    if !parsed.body.contains("{term}") {
        anyhow::bail!("Prompt is missing required placeholder: {{term}}");
    }
    if !parsed.body.contains("{count}") {
        anyhow::bail!("Prompt is missing required placeholder: {{count}}");
    }

    eprintln!("Loaded prompt for deck: {}", frontmatter.deck);
    eprintln!("Note type: {}", frontmatter.note_type);

    // 2. Validate Anki assets
    let anki = AnkiClient::new();
    eprintln!("\nValidating Anki configuration...");
    let validation = validate_anki_assets(&anki, &frontmatter)?;
    eprintln!(
        "Note type fields: {}",
        validation.note_type_fields.join(", ")
    );

    // 3. Build logger
    let logger = LlmLogger::new(args.log.as_deref(), args.very_verbose)?;

    // 4. Resolve LLM config
    let runtime = build_runtime_config(RuntimeConfigArgs {
        model: args.model.as_deref(),
        batch_size: None,
        max_tokens: args.max_tokens,
        temperature: args.temperature,
        retries: args.retries,
        dry_run: false,
    })?;

    // 5. Generate cards
    let field_map_keys: Vec<String> = frontmatter.field_map.keys().cloned().collect();
    let mut generation_cost = 0.0;
    let candidates: Vec<CardCandidate>;

    let client = if args.copy {
        None
    } else {
        Some(LlmClient::from_config(&runtime))
    };

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
                                "  Warning: Response is missing field \"{key}\". Skipping card."
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
            eprintln!("Skipped {} card(s) due to missing fields.", skipped);
        }
        eprintln!("Parsed {} card(s) from response", candidates.len());
    } else {
        let client = client.as_ref().unwrap();

        let spinner = crate::spinner::llm_spinner(format!(
            "Generating {} card(s) for \"{}\" using {}...",
            args.count, term, runtime.model
        ));

        let result = generate_cards(
            &term,
            &parsed.body,
            args.count,
            &field_map_keys,
            None,
            client,
            &runtime.model,
            runtime.temperature,
            runtime.max_tokens,
            runtime.retries,
            Some(&logger),
            on_log,
        )?;
        spinner.finish_and_clear();

        if let Some(ref cost) = result.cost {
            generation_cost = cost.total_cost;
            eprintln!(
                "  Tokens: {} in / {} out | Cost: {}",
                cost.input_tokens,
                cost.output_tokens,
                pricing::format_cost(cost.total_cost)
            );
        }

        candidates = result.cards;

        if candidates.is_empty() {
            anyhow::bail!("No cards were generated");
        }

        eprintln!("Generated {} card(s)", candidates.len());

        if candidates.len() != args.count as usize {
            eprintln!(
                "  Warning: Requested {} cards, received {}",
                args.count,
                candidates.len()
            );
        }
    }

    // 5. Sanitize and validate
    eprintln!("\nChecking for duplicates...");

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
        eprintln!("Found {dup_count} duplicate(s) (already in Anki)");
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
        eprintln!("\nSkipping {dup_selected} duplicate(s) — already exist in Anki.");
        selected.retain(|c| !c.is_duplicate);
    }

    if selected.is_empty() {
        eprintln!("No non-duplicate cards selected. Exiting.");
        return Ok(());
    }

    // 7. Quality check
    let qc_result = if let Some(ref client) = client {
        let spinner =
            crate::spinner::llm_spinner(format!("Quality check using {}...", runtime.model));

        let result = run_quality_checks(
            selected,
            frontmatter.quality_check.as_ref(),
            client,
            &runtime.model,
            runtime.temperature,
            runtime.max_tokens,
            runtime.retries,
            Some(&logger),
            on_log,
        )?;

        spinner.finish_and_clear();
        result
    } else {
        super::quality::QualityRunResult {
            passed: selected,
            flagged: vec![],
            cost: 0.0,
            input_tokens: 0,
            output_tokens: 0,
        }
    };

    // Interactive review of flagged cards (legacy mode)
    let mut final_cards = qc_result.passed;

    if !qc_result.flagged.is_empty() {
        let flagged_count = qc_result.flagged.len();
        eprintln!("\n{flagged_count} card(s) were flagged. Please review:");

        for (i, flagged) in qc_result.flagged.into_iter().enumerate() {
            eprintln!("\n--- Flagged Card {}/{} ---", i + 1, flagged_count);
            for (key, value) in &flagged.card.fields {
                eprintln!("{key}: {value}");
            }
            eprintln!("\nReason: {}", flagged.reason);

            let keep = inquire::Confirm::new("Keep this card anyway?")
                .with_default(false)
                .prompt()
                .unwrap_or(false);

            if keep {
                final_cards.push(flagged.card);
            }
        }
    }

    if final_cards.is_empty() {
        eprintln!("\nNo cards remaining after quality check. Exiting.");
        return Ok(());
    }

    let total_cost = generation_cost + qc_result.cost;
    if total_cost > 0.0 {
        eprintln!(
            "\nTotal estimated cost: {}",
            pricing::format_cost(total_cost)
        );
    }

    // 8. Export or import
    if let Some(ref output_path) = args.output {
        export_cards(&final_cards, output_path, on_log)?;
    } else {
        let result = import_cards_to_anki(&final_cards, &frontmatter, &anki, on_log)?;
        report_import_result(&result, &frontmatter.deck, on_log);

        if result.failures > 0 {
            anyhow::bail!(
                "Import failed: {} card(s) could not be added. Check your Anki collection and try again.",
                result.failures
            );
        }
    }

    Ok(())
}

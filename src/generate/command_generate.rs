use std::sync::mpsc;

use anyhow::Result;

use crate::anki::client::AnkiClient;
use crate::cli::GenerateArgs;
use crate::llm::client::LlmClient;
use crate::llm::logger::LlmLogger;
use crate::llm::parse_json::try_parse_json_array;
use crate::llm::provider::available_models;
use crate::llm::runtime::{RuntimeConfig, RuntimeConfigArgs, build_runtime_config};
use crate::template::frontmatter::{Frontmatter, parse_prompt_file};

use crate::style::style;

use super::anki_import::{import_cards_to_anki, report_import_result};
use super::cards::ValidatedCard;
use super::exporter::export_cards;
use super::manual::get_llm_response_manually;
use super::pipeline::{
    PipelineConfig, PipelineInteraction, PipelineOutcome, PipelineProgress, PipelineStep,
    ReviewResult, SelectionAction,
};
use super::process::FlaggedCard;
use super::processor::CardCandidate;
use super::selector::{display_cards, select_cards_legacy};
use super::tui::{BackendEvent, SessionInfo, StepStatus, WorkerCommand};
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
// TUI adapter
// ---------------------------------------------------------------------------

struct TuiProgress {
    tx: mpsc::Sender<BackendEvent>,
}

impl PipelineProgress for TuiProgress {
    fn log(&self, msg: &str) {
        self.tx.send(BackendEvent::Log(msg.to_string())).ok();
    }

    fn step_start(&self, step: PipelineStep, _detail: Option<&str>) {
        self.tx
            .send(BackendEvent::StepUpdate {
                step,
                status: StepStatus::Running(None),
            })
            .ok();
    }

    fn step_done(&self, step: PipelineStep, detail: Option<String>) {
        self.tx
            .send(BackendEvent::StepUpdate {
                step,
                status: StepStatus::Done(detail),
            })
            .ok();
    }

    fn step_skip(&self, step: PipelineStep) {
        self.tx
            .send(BackendEvent::StepUpdate {
                step,
                status: StepStatus::Skipped,
            })
            .ok();
    }

    fn step_error(&self, step: PipelineStep, detail: &str) {
        self.tx
            .send(BackendEvent::StepUpdate {
                step,
                status: StepStatus::Error(detail.to_string()),
            })
            .ok();
    }

    fn cost_update(&self, input_tokens: u64, output_tokens: u64, cost: f64) {
        self.tx
            .send(BackendEvent::CostUpdate {
                input_tokens,
                output_tokens,
                cost,
            })
            .ok();
    }
}

struct TuiInteraction<'a> {
    tx: mpsc::Sender<BackendEvent>,
    rx: &'a mpsc::Receiver<WorkerCommand>,
}

impl PipelineInteraction for TuiInteraction<'_> {
    fn begin_selection(&self, cards: Vec<ValidatedCard>) {
        self.tx.send(BackendEvent::RequestSelection(cards)).ok();
    }

    fn append_selection(&self, cards: Vec<ValidatedCard>) {
        self.tx.send(BackendEvent::AppendCards(cards)).ok();
    }

    fn wait_selection(&self) -> SelectionAction {
        match self.rx.recv() {
            Ok(WorkerCommand::Refresh) => SelectionAction::Refresh,
            Ok(WorkerCommand::Selection(indices)) => SelectionAction::Selected(indices),
            Ok(WorkerCommand::Cancel) => SelectionAction::Cancel,
            Ok(WorkerCommand::Quit) | Err(_) => SelectionAction::Quit,
            _ => SelectionAction::Cancel,
        }
    }

    fn request_review(&self, flagged: Vec<FlaggedCard>) -> ReviewResult {
        self.tx.send(BackendEvent::RequestReview(flagged)).ok();
        match self.rx.recv() {
            Ok(WorkerCommand::Review(decisions)) => ReviewResult::Reviewed(decisions),
            _ => ReviewResult::Cancel,
        }
    }
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

                let progress = TuiProgress { tx: tx.clone() };
                let interaction = TuiInteraction {
                    tx: tx.clone(),
                    rx: &rx,
                };

                let config = PipelineConfig {
                    frontmatter: &session.frontmatter,
                    prompt_body: &session.prompt_body,
                    field_map_keys: &session.field_map_keys,
                    validation: &session.validation,
                    client: &session.client,
                    anki: &session.anki,
                    logger: &session.logger,
                    model: &session.runtime.model,
                    temperature: session.runtime.temperature,
                    max_tokens: session.runtime.max_tokens,
                    retries: session.runtime.retries,
                    count: args.count,
                    dry_run: args.dry_run,
                    output: args.output.as_deref(),
                };

                match super::pipeline::run_pipeline_for_term(
                    &config,
                    &interaction,
                    &progress,
                    &term,
                    &[],
                ) {
                    Ok(PipelineOutcome::Success { message, cards, note_ids }) => {
                        tx.send(BackendEvent::RunDone { message, cards, note_ids }).ok();
                    }
                    Ok(PipelineOutcome::Cancelled) | Ok(PipelineOutcome::Quit) => {
                        // Send RunDone so the TUI can clear its pending_cancels counter
                        tx.send(BackendEvent::RunDone { message: String::new(), cards: Vec::new(), note_ids: Vec::new() }).ok();
                    }
                    Err(e) => {
                        tx.send(BackendEvent::RunError(format!("{e}"))).ok();
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

// ---------------------------------------------------------------------------
// Legacy adapter
// ---------------------------------------------------------------------------

struct LegacyProgress;

impl PipelineProgress for LegacyProgress {
    fn log(&self, msg: &str) {
        eprintln!("{msg}");
    }

    fn step_start(&self, _step: PipelineStep, _detail: Option<&str>) {}
    fn step_done(&self, _step: PipelineStep, _detail: Option<String>) {}
    fn step_skip(&self, _step: PipelineStep) {}
    fn step_error(&self, _step: PipelineStep, _detail: &str) {}
    fn cost_update(&self, _input_tokens: u64, _output_tokens: u64, _cost: f64) {}
}

struct LegacyInteraction {
    cards: std::cell::RefCell<Vec<ValidatedCard>>,
}

impl PipelineInteraction for LegacyInteraction {
    fn begin_selection(&self, cards: Vec<ValidatedCard>) {
        *self.cards.borrow_mut() = cards;
    }

    fn append_selection(&self, _cards: Vec<ValidatedCard>) {
        unreachable!("Legacy mode does not support refresh");
    }

    fn wait_selection(&self) -> SelectionAction {
        let cards = self.cards.borrow();
        match select_cards_legacy(&cards) {
            Ok(indices) => SelectionAction::Selected(indices),
            Err(_) => SelectionAction::Cancel,
        }
    }

    fn request_review(&self, flagged: Vec<FlaggedCard>) -> ReviewResult {
        let flagged_count = flagged.len();
        eprintln!("\n{flagged_count} card(s) were flagged. Please review:");

        let mut decisions = Vec::with_capacity(flagged_count);
        for (i, fc) in flagged.iter().enumerate() {
            eprintln!("\n--- Flagged Card {}/{} ---", i + 1, flagged_count);
            for (key, value) in &fc.card.fields {
                eprintln!("{key}: {value}");
            }
            eprintln!("\nReason: {}", fc.reason);

            let keep = inquire::Confirm::new("Keep this card anyway?")
                .with_default(false)
                .prompt()
                .unwrap_or(false);
            decisions.push(keep);
        }

        ReviewResult::Reviewed(decisions)
    }
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

    let field_map_keys: Vec<String> = frontmatter.field_map.keys().cloned().collect();
    let client = runtime.as_ref().map(LlmClient::from_config);
    let on_log: &(dyn Fn(&str) + Send + Sync) = &|msg| eprintln!("{msg}");

    if args.copy {
        // Manual copy-paste mode — NOT routed through pipeline
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
        let candidates: Vec<CardCandidate> = parsed_arr
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

        // Sanitize and validate
        let sanitized_pairs: Vec<_> = candidates
            .into_iter()
            .map(|c| {
                let s = super::sanitize::sanitize_fields(&c.fields);
                (c, s)
            })
            .collect();

        let first_field_name = &validation.note_type_fields[0];
        let validated =
            super::cards::validate_cards(sanitized_pairs, &frontmatter, first_field_name, &anki)?;

        let dup_count = validated.iter().filter(|c| c.is_duplicate).count();
        if dup_count > 0 {
            eprintln!(
                "  {}",
                s.muted(format!("{dup_count} duplicate(s) already in Anki"))
            );
        }

        if args.dry_run {
            display_cards(&validated);
            return Ok(());
        }

        if validated.is_empty() {
            eprintln!("No cards to select from.");
            return Ok(());
        }

        let selected_indices = select_cards_legacy(&validated)?;
        let selected: Vec<ValidatedCard> = selected_indices
            .iter()
            .filter_map(|&i| validated.get(i).cloned())
            .collect();

        if selected.is_empty() {
            eprintln!("\nNo cards selected. Exiting.");
            return Ok(());
        }

        // Export or import
        if let Some(ref output_path) = args.output {
            export_cards(&selected, output_path, on_log)?;
        } else {
            let result = import_cards_to_anki(&selected, &frontmatter, &anki, on_log)?;
            report_import_result(&result, &frontmatter.deck);

            if result.failures > 0 {
                anyhow::bail!(
                    "Import failed: {} card(s) could not be added. Check your Anki collection and try again.",
                    result.failures
                );
            }
        }

        return Ok(());
    }

    // Non-copy legacy mode — route through shared pipeline
    let rt = runtime.as_ref().unwrap();
    let client_ref = client.as_ref().unwrap();

    eprintln!("  {}  {}", s.muted("Model    "), s.muted(&rt.model));

    let config = PipelineConfig {
        frontmatter: &frontmatter,
        prompt_body: &parsed.body,
        field_map_keys: &field_map_keys,
        validation: &validation,
        client: client_ref,
        anki: &anki,
        logger: &logger,
        model: &rt.model,
        temperature: rt.temperature,
        max_tokens: rt.max_tokens,
        retries: rt.retries,
        count: args.count,
        dry_run: args.dry_run,
        output: args.output.as_deref(),
    };

    let progress = LegacyProgress;
    let interaction = LegacyInteraction {
        cards: std::cell::RefCell::new(Vec::new()),
    };

    match super::pipeline::run_pipeline_for_term(&config, &interaction, &progress, &term, &[])? {
        PipelineOutcome::Success { message } => {
            if !message.is_empty() {
                eprintln!("\n  {}", s.green(&message));
            }
            Ok(())
        }
        PipelineOutcome::Cancelled => {
            eprintln!("\nCancelled.");
            Ok(())
        }
        PipelineOutcome::Quit => {
            eprintln!("\nQuit.");
            Ok(())
        }
    }
}

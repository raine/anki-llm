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

use crate::style::{Style, style};

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

struct LoadedPrompt {
    frontmatter: Frontmatter,
    body: String,
}

struct PreparedSession {
    frontmatter: Frontmatter,
    prompt_body: String,
    validation: ValidationResult,
    runtime: RuntimeConfig,
    field_map_keys: Vec<String>,
    anki: AnkiClient,
    client: LlmClient,
    logger: LlmLogger,
    /// Bundled TTS service + media store, built once per session when the
    /// prompt has a `tts:` block. Shared by the (future) preview path and
    /// the import-time finalizer so both hit the same cache and the same
    /// upload-dedup map.
    tts: Option<crate::tts::service::TtsBundle>,
}

/// Load and parse a prompt file, validating required placeholders.
fn load_prompt(args: &GenerateArgs) -> Result<LoadedPrompt> {
    let prompt_path = require_prompt_path(&args.prompt)?;
    crate::workspace::resolver::save_last_prompt(&prompt_path);
    let content = std::fs::read_to_string(&prompt_path)?;
    let parsed = parse_prompt_file(&content)?;

    if !parsed.body.contains("{term}") {
        anyhow::bail!("Prompt is missing required placeholder: {{term}}");
    }
    if !parsed.body.contains("{count}") {
        anyhow::bail!("Prompt is missing required placeholder: {{count}}");
    }

    Ok(LoadedPrompt {
        frontmatter: parsed.frontmatter,
        body: parsed.body,
    })
}

/// Full session setup: load prompt, validate Anki, build logger/runtime/client.
/// Reports progress via the `PipelineProgress` trait (TUI shows steps, legacy no-ops).
fn prepare_session(
    args: &GenerateArgs,
    very_verbose: bool,
    progress: &dyn PipelineProgress,
) -> Result<PreparedSession> {
    progress.step_start(PipelineStep::LoadPrompt, None);
    let loaded = load_prompt(args).inspect_err(|e| {
        progress.step_error(PipelineStep::LoadPrompt, &e.to_string());
    })?;
    progress.step_done(PipelineStep::LoadPrompt, None);

    progress.step_start(PipelineStep::ValidateAnki, None);
    let anki = AnkiClient::new();
    let validation = validate_anki_assets(&anki, &loaded.frontmatter).inspect_err(|e| {
        progress.step_error(PipelineStep::ValidateAnki, &e.to_string());
    })?;
    progress.step_done(PipelineStep::ValidateAnki, None);

    let logger = LlmLogger::new(args.log.as_deref(), very_verbose)?;
    let runtime = build_runtime_config(RuntimeConfigArgs {
        model: args.model.as_deref(),
        api_base_url: args.api_base_url.as_deref(),
        api_key: args.api_key.as_deref(),
        batch_size: None,
        max_tokens: args.max_tokens,
        temperature: args.temperature,
        retries: args.retries,
        dry_run: false,
    })?;
    let field_map_keys: Vec<String> = loaded.frontmatter.field_map.keys().cloned().collect();
    let client = LlmClient::from_config(&runtime);

    let tts = if let Some(ref spec) = loaded.frontmatter.tts {
        // TTS credentials are resolved from env/config (`AZURE_TTS_KEY`,
        // `OPENAI_API_KEY`, `~/.config/anki-llm/config.toml`'s `tts_*` fields),
        // not from `--api-key`/`--api-base-url` which are LLM-only transport
        // flags and may legitimately point at OpenRouter, Ollama, etc.
        let bundle = crate::tts::service::build_bundle(
            spec,
            AnkiClient::new(),
            crate::tts::service::TtsBundleOptions {
                api_key: None,
                api_base_url: None,
                azure_region: None,
            },
        )
        .inspect_err(|e| {
            progress.step_error(PipelineStep::ValidateAnki, &e.to_string());
        })?;
        Some(bundle)
    } else {
        None
    };

    Ok(PreparedSession {
        frontmatter: loaded.frontmatter,
        prompt_body: loaded.body,
        validation,
        runtime,
        field_map_keys,
        anki,
        client,
        logger,
        tts,
    })
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

    fn replace_card(&self, previous_card_id: u64, card: ValidatedCard) {
        self.tx
            .send(BackendEvent::ReplaceCard {
                previous_card_id,
                card,
            })
            .ok();
    }

    fn regen_error(&self, target_id: u64, message: String) {
        self.tx
            .send(BackendEvent::RegenError { target_id, message })
            .ok();
    }

    fn wait_selection(&self) -> SelectionAction {
        match self.rx.recv() {
            Ok(WorkerCommand::Refresh) => SelectionAction::Refresh,
            Ok(WorkerCommand::RefreshWithTerm(term)) => SelectionAction::RefreshWithTerm(term),
            Ok(WorkerCommand::RegenerateCard { card, feedback }) => {
                SelectionAction::RegenerateCard { card, feedback }
            }
            Ok(WorkerCommand::PreviewTts { card }) => SelectionAction::PreviewTts { card },
            Ok(WorkerCommand::Selection(cards)) => SelectionAction::Selected(cards),
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

    fn tts_state(&self, card_id: u64, state: crate::generate::tui::events::TtsUiState) {
        self.tx.send(BackendEvent::TtsState { card_id, state }).ok();
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

    // --- Session setup (done once) ---

    let progress = TuiProgress { tx: tx.clone() };
    // Disable very_verbose in TUI mode — raw stderr output would corrupt the display.
    let mut session = prepare_session(&args, false, &progress).map_err(|e| {
        tx.send(BackendEvent::Fatal(format!("{e}"))).ok();
        e
    })?;

    log!("Loaded prompt for deck: {}", session.frontmatter.deck);
    log!("Note type: {}", session.frontmatter.note_type);
    log!(
        "Note type fields: {}",
        session.validation.note_type_fields.join(", ")
    );

    let models: Vec<String> = available_models(args.dry_run);

    // Worker-side only knows whether the YAML declares `tts:`. The TUI
    // main thread owns audio-backend detection and player ownership,
    // and combines this flag with its own detection result to decide
    // whether to show the preview keybind.
    let tts_configured = session.tts.is_some();

    tx.send(BackendEvent::SessionReady(SessionInfo {
        deck: session.frontmatter.deck.clone(),
        note_type: session.frontmatter.note_type.clone(),
        model: session.runtime.model.clone(),
        available_models: models.clone(),
        field_map: session.frontmatter.field_map.clone(),
        tts_preview_enabled: tts_configured,
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
                    tts: session.tts.as_ref(),
                };

                match super::pipeline::run_pipeline_for_term(
                    &config,
                    &interaction,
                    &progress,
                    &term,
                    &[],
                ) {
                    Ok(PipelineOutcome::Success {
                        message,
                        cards,
                        note_ids,
                    }) => {
                        tx.send(BackendEvent::RunDone {
                            message,
                            cards,
                            note_ids,
                        })
                        .ok();
                    }
                    Ok(PipelineOutcome::Cancelled) | Ok(PipelineOutcome::Quit) => {
                        // Send RunDone so the TUI can clear its pending_cancels counter
                        tx.send(BackendEvent::RunDone {
                            message: String::new(),
                            cards: Vec::new(),
                            note_ids: Vec::new(),
                        })
                        .ok();
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
                    api_base_url: args.api_base_url.as_deref(),
                    api_key: args.api_key.as_deref(),
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
                            field_map: session.frontmatter.field_map.clone(),
                            tts_preview_enabled: tts_configured,
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

    fn replace_card(&self, _previous_card_id: u64, _card: ValidatedCard) {
        unreachable!("Legacy mode does not support regeneration");
    }

    fn regen_error(&self, _target_id: u64, _message: String) {
        unreachable!("Legacy mode does not support regeneration");
    }

    fn wait_selection(&self) -> SelectionAction {
        let cards = self.cards.borrow();
        match select_cards_legacy(&cards) {
            Ok(indices) => {
                // Map indices to cloned cards so the new
                // `Selected(Vec<ValidatedCard>)` shape is honored. The
                // legacy interactive selector still works on indices
                // internally; we just adapt at the boundary.
                let selected: Vec<ValidatedCard> = indices
                    .into_iter()
                    .filter_map(|i| cards.get(i).cloned())
                    .collect();
                SelectionAction::Selected(selected)
            }
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

    let s = style();
    let on_log: &(dyn Fn(&str) + Send + Sync) = &|msg| eprintln!("{msg}");

    if args.copy {
        return run_copy_mode(&args, &term, s, on_log);
    }

    // Non-copy legacy mode — full session setup, route through shared pipeline
    let session = prepare_session(&args, args.very_verbose, &LegacyProgress)?;

    eprintln!(
        "  {}  {}",
        s.muted("Deck     "),
        s.cyan(&session.frontmatter.deck)
    );
    eprintln!(
        "  {}  {}",
        s.muted("Note type"),
        s.cyan(&session.frontmatter.note_type)
    );
    eprintln!(
        "  {}  {}",
        s.muted("Fields   "),
        s.muted(session.validation.note_type_fields.join(", "))
    );
    eprintln!(
        "  {}  {}",
        s.muted("Model    "),
        s.muted(&session.runtime.model)
    );

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
        tts: session.tts.as_ref(),
    };

    let interaction = LegacyInteraction {
        cards: std::cell::RefCell::new(Vec::new()),
    };

    match super::pipeline::run_pipeline_for_term(
        &config,
        &interaction,
        &LegacyProgress,
        &term,
        &[],
    )? {
        PipelineOutcome::Success { message, .. } => {
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

/// Manual copy-paste mode — loads prompt and Anki config but skips LLM client setup.
fn run_copy_mode(
    args: &GenerateArgs,
    term: &str,
    s: &Style,
    on_log: &(dyn Fn(&str) + Send + Sync),
) -> Result<()> {
    let loaded = load_prompt(args)?;
    let frontmatter = &loaded.frontmatter;

    let has_processing = frontmatter
        .processing
        .as_ref()
        .map(|p| !p.pre_select.is_empty() || !p.post_select.is_empty())
        .unwrap_or(false);
    if has_processing {
        anyhow::bail!("processing is not supported in --copy mode");
    }

    eprintln!("  {}  {}", s.muted("Deck     "), s.cyan(&frontmatter.deck));
    eprintln!(
        "  {}  {}",
        s.muted("Note type"),
        s.cyan(&frontmatter.note_type)
    );

    let anki = AnkiClient::new();
    let validation = validate_anki_assets(&anki, frontmatter)?;
    eprintln!(
        "  {}  {}",
        s.muted("Fields   "),
        s.muted(validation.note_type_fields.join(", "))
    );

    let field_map_keys: Vec<String> = frontmatter.field_map.keys().cloned().collect();

    let mut row = crate::data::Row::new();
    row.insert("term".into(), serde_json::Value::String(term.to_string()));
    row.insert(
        "count".into(),
        serde_json::Value::String(args.count.to_string()),
    );
    let filled = crate::template::fill_template(&loaded.body, &row)?;
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
        super::cards::validate_cards(sanitized_pairs, frontmatter, first_field_name, &anki, "")?;

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
    let mut selected: Vec<ValidatedCard> = selected_indices
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
        let tts_bundle = if let Some(ref spec) = frontmatter.tts {
            // TTS credentials come from env/config, not from generate's
            // `--api-key`/`--api-base-url` which target the LLM endpoint.
            Some(crate::tts::service::build_bundle(
                spec,
                AnkiClient::new(),
                crate::tts::service::TtsBundleOptions {
                    api_key: None,
                    api_base_url: None,
                    azure_region: None,
                },
            )?)
        } else {
            None
        };
        let tts_finalize = tts_bundle
            .as_ref()
            .map(|b| crate::generate::anki_import::TtsFinalize {
                service: &b.service,
                media: &b.media,
            });
        let result = import_cards_to_anki(&mut selected, frontmatter, &anki, tts_finalize, on_log)?;
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

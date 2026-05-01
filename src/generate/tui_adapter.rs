use std::sync::mpsc;

use anyhow::Result;

use crate::cli::GenerateArgs;
use crate::llm::client::LlmClient;
use crate::llm::provider::available_models;
use crate::llm::runtime::{RuntimeConfigArgs, build_runtime_config};

use super::cards::ValidatedCard;
use super::pipeline::{
    PipelineConfig, PipelineInteraction, PipelineOutcome, PipelineProgress, PipelineStep,
    ReviewResult, SelectionAction,
};
use super::process::FlaggedCard;
use super::session::prepare_session;
use super::tui::{BackendEvent, SessionInfo, StepStatus, WorkerCommand};

pub(super) struct TuiProgress {
    pub tx: mpsc::Sender<BackendEvent>,
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

    fn thinking_reset(&self) {
        self.tx.send(BackendEvent::ThinkingReset).ok();
    }

    fn thinking_delta(&self, delta: &str) {
        self.tx
            .send(BackendEvent::ThinkingDelta(delta.to_string()))
            .ok();
    }

    fn thinking_clear(&self) {
        self.tx.send(BackendEvent::ThinkingClear).ok();
    }
}

pub(super) struct TuiInteraction<'a> {
    pub tx: mpsc::Sender<BackendEvent>,
    pub rx: &'a mpsc::Receiver<WorkerCommand>,
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
            Ok(WorkerCommand::Selection {
                cards,
                skip_post_select,
            }) => SelectionAction::Selected {
                cards,
                skip_post_select,
            },
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
    // whether to show the preview keybind. This is derived from the
    // spec's presence, not from a materialized bundle — the bundle is
    // lazily built on first use (preview or import).
    let tts_configured = session.tts.is_some();
    let post_select_configured = session
        .frontmatter
        .processing
        .as_ref()
        .is_some_and(|p| !p.post_select.is_empty());

    tx.send(BackendEvent::SessionReady(SessionInfo {
        deck: session.frontmatter.deck.clone(),
        note_type: session.frontmatter.note_type.clone(),
        model: session.runtime.model.clone(),
        available_models: models.clone(),
        field_map: session.frontmatter.field_map.clone(),
        first_field_name: session.validation.note_type_fields[0].clone(),
        tts_configured,
        post_select_configured,
    }))
    .ok();

    // --- Per-term loop ---
    loop {
        match rx.recv() {
            Ok(WorkerCommand::Start {
                term,
                enable_thinking_stream,
            }) => {
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
                    enable_thinking_stream,
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
                        failed,
                    }) => {
                        tx.send(BackendEvent::RunDone {
                            message,
                            cards,
                            note_ids,
                            failed,
                        })
                        .ok();
                    }
                    Ok(PipelineOutcome::Cancelled) | Ok(PipelineOutcome::Quit) => {
                        // Send RunDone so the TUI can clear its pending_cancels counter
                        tx.send(BackendEvent::RunDone {
                            message: String::new(),
                            cards: Vec::new(),
                            note_ids: Vec::new(),
                            failed: false,
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
                            first_field_name: session.validation.note_type_fields[0].clone(),
                            tts_configured,
                            post_select_configured,
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

use anyhow::Result;

use crate::cli::GenerateArgs;
use crate::style::style;

use super::cards::ValidatedCard;
use super::copy_mode::run_copy_mode;
use super::pipeline::{
    PipelineConfig, PipelineInteraction, PipelineOutcome, PipelineProgress, PipelineStep,
    ReviewResult, SelectionAction,
};
use super::process::FlaggedCard;
use super::selector::select_cards_legacy;
use super::session::prepare_session;

pub(super) struct LegacyProgress;

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
                SelectionAction::Selected {
                    cards: selected,
                    skip_post_select: false,
                }
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

pub(super) fn run_legacy(args: GenerateArgs) -> Result<()> {
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
        enable_thinking_stream: false,
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
        PipelineOutcome::Success {
            message, failed, ..
        } => {
            if failed {
                // Non-TUI generate has no Done view to recover cards
                // from, so surface the failure as a non-zero exit so
                // shell scripts and batch runners see it.
                anyhow::bail!(
                    "{}",
                    if message.is_empty() {
                        "import failed".to_string()
                    } else {
                        message
                    }
                );
            }
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

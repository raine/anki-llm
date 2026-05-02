//! Card generation pipeline. Orchestrates the generate → validate →
//! select → post-select → finalize stages. Concrete stage logic lives
//! in the sibling submodules; this file is a thin facade that exposes
//! the public types/traits and the `run_pipeline_for_term` entry point
//! to the rest of the crate.

mod finalize;
mod generation;
mod post_select;
mod preview;
mod regenerate;
mod types;

use anyhow::Result;

pub use types::{
    PipelineConfig, PipelineInteraction, PipelineOutcome, PipelineProgress, PipelineStep,
    ReviewResult, SelectionAction, TtsPreviewState,
};

use finalize::{run_export, run_finalize_tts_step, run_import_step};
use generation::{GenerateLoopResult, run_generate_select_loop};
use post_select::{PostSelectInput, PostSelectResult, run_post_select};

/// Drive a full generation run for a single term. Returns when the
/// user's curated cards have been imported / exported, or when the
/// user cancels or quits.
pub fn run_pipeline_for_term(
    config: &PipelineConfig,
    interaction: &dyn PipelineInteraction,
    progress: &dyn PipelineProgress,
    term: &str,
    exclude_terms: &[String],
) -> Result<PipelineOutcome> {
    let on_log: &(dyn Fn(&str) + Send + Sync) = &|msg: &str| {
        progress.log(msg);
    };

    let (cards, skip_post_select) =
        match run_generate_select_loop(config, interaction, progress, term, exclude_terms)? {
            GenerateLoopResult::Selected {
                cards,
                skip_post_select,
            } => (cards, skip_post_select),
            GenerateLoopResult::EarlyOutcome(outcome) => return Ok(outcome),
        };

    let mut final_cards = match run_post_select(
        config,
        interaction,
        progress,
        PostSelectInput {
            cards,
            skip_post_select,
        },
        on_log,
    )? {
        PostSelectResult::Continue(cards) => cards,
        PostSelectResult::Outcome(outcome) => return Ok(outcome),
    };

    if let Some(output_path) = config.output {
        return run_export(final_cards, output_path, progress, on_log);
    }

    if let Err(outcome) = run_finalize_tts_step(
        &mut final_cards,
        config.tts,
        config.frontmatter,
        progress,
        on_log,
    ) {
        return Ok(outcome);
    }

    Ok(run_import_step(
        final_cards,
        config.frontmatter,
        config.anki,
        progress,
        on_log,
    ))
}

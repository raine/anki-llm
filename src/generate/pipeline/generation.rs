use std::collections::HashSet;

use anyhow::Result;

use crate::llm::pricing;

use super::super::cards::{ValidatedCard, validate_cards};
use super::super::process::{CardFlag, run_processors};
use super::super::processor::{CardCandidate, generate_cards};
use super::super::sanitize::sanitize_fields;
use super::super::selector::strip_html_tags;
use super::regenerate::wait_selection_with_regen;
use super::types::{
    PipelineConfig, PipelineInteraction, PipelineOutcome, PipelineProgress, PipelineStep,
    SelectionAction,
};

pub(super) enum GenerateLoopResult {
    Selected {
        cards: Vec<ValidatedCard>,
        skip_post_select: bool,
    },
    EarlyOutcome(PipelineOutcome),
}

/// Run the generate / pre-process / validate / select loop for a term.
/// The loop supports the user issuing `Refresh` / `RefreshWithTerm`
/// actions, which append additional candidates to the existing
/// selection. Returns either the user's terminal selection or an
/// outcome that should short-circuit the rest of the pipeline.
pub(super) fn run_generate_select_loop(
    config: &PipelineConfig,
    interaction: &dyn PipelineInteraction,
    progress: &dyn PipelineProgress,
    term: &str,
    exclude_terms: &[String],
) -> Result<GenerateLoopResult> {
    let first_field_name = &config.validation.note_type_fields[0];
    let on_log: &(dyn Fn(&str) + Send + Sync) = &|msg: &str| {
        progress.log(msg);
    };

    let mut seen_keys: HashSet<String> = HashSet::new();
    let mut is_refresh = false;
    let mut current_term = term.to_string();

    // The worker holds no card state between iterations: each batch's
    // validated cards are handed to the TUI via `begin_selection` /
    // `append_selection`, and the TUI is the source of truth for every
    // mutation (selection, edit, remove, force-toggle) until the user
    // submits. `seen_keys` is the only worker-side state that survives
    // a refresh, and it's purely a normalized-text dedup set for the
    // LLM exclude list — it never indexes into a card collection.

    loop {
        let mut exclude: Vec<String> = seen_keys.iter().cloned().collect();
        exclude.sort();
        exclude.extend_from_slice(exclude_terms);

        if is_refresh {
            progress.step_done(PipelineStep::Select, None);
        }
        progress.step_start(PipelineStep::Generate, None);
        if is_refresh {
            progress.log(&format!(
                "Generating {} more card(s) for \"{}\"...",
                config.count, current_term,
            ));
        } else {
            progress.log(&format!(
                "Generating {} card(s) for \"{}\" using {}...",
                config.count, current_term, config.model
            ));
        }

        let gen_result = generate_cards(
            &current_term,
            config.prompt_body,
            config.count,
            config.field_map_keys,
            if exclude.is_empty() {
                None
            } else {
                Some(&exclude)
            },
            config.client,
            config.model,
            config.temperature,
            config.max_tokens,
            config.retries,
            Some(config.logger),
            on_log,
            if !is_refresh && config.enable_thinking_stream {
                Some(progress)
            } else {
                None
            },
        );

        let gen_result = match gen_result {
            Ok(r) => r,
            Err(e) => {
                if is_refresh {
                    progress.log(&format!("Refresh failed: {e}"));
                    interaction.append_selection(Vec::new());
                    progress.step_error(PipelineStep::Generate, &format!("{e}"));

                    match handle_refresh_wait(
                        config,
                        interaction,
                        progress,
                        &mut is_refresh,
                        &mut current_term,
                    ) {
                        RefreshWaitOutcome::Continue => continue,
                        RefreshWaitOutcome::Selected {
                            cards,
                            skip_post_select,
                        } => {
                            return Ok(GenerateLoopResult::Selected {
                                cards,
                                skip_post_select,
                            });
                        }
                        RefreshWaitOutcome::Outcome(outcome) => {
                            return Ok(GenerateLoopResult::EarlyOutcome(outcome));
                        }
                    }
                } else {
                    progress.step_error(PipelineStep::Generate, &format!("{e}"));
                    return Err(e);
                }
            }
        };

        if let Some(ref cost) = gen_result.cost {
            progress.cost_update(cost.input_tokens, cost.output_tokens, cost.total_cost);
            progress.log(&format!(
                "Tokens: {} in / {} out | Cost: {}",
                cost.input_tokens,
                cost.output_tokens,
                pricing::format_cost(cost.total_cost)
            ));
        }

        let mut candidates = gen_result.cards;

        if candidates.is_empty() && !is_refresh {
            return Err(anyhow::anyhow!("No cards were generated"));
        }

        progress.step_done(PipelineStep::Generate, None);
        progress.log(&format!("Generated {} card(s)", candidates.len()));
        if !candidates.is_empty() && candidates.len() != config.count as usize {
            progress.log(&format!(
                "Warning: requested {} cards, received {}",
                config.count,
                candidates.len()
            ));
        }

        let pre_select_flags = run_pre_select(config, &mut candidates, progress, on_log)?;

        // Sanitize and validate
        progress.step_start(PipelineStep::Validate, None);
        progress.log("Checking for duplicates...");

        let sanitized_pairs: Vec<_> = candidates
            .into_iter()
            .map(|c| {
                let s = sanitize_fields(&c.fields);
                (c, s)
            })
            .collect();

        let validated = match validate_cards(
            sanitized_pairs,
            config.frontmatter,
            first_field_name,
            config.anki,
            config.model,
        ) {
            Ok(v) => v,
            Err(e) => {
                if is_refresh {
                    progress.log(&format!("Validation failed during refresh: {e}"));
                    interaction.append_selection(Vec::new());
                    progress.step_error(PipelineStep::Validate, &format!("{e}"));

                    match handle_refresh_wait(
                        config,
                        interaction,
                        progress,
                        &mut is_refresh,
                        &mut current_term,
                    ) {
                        RefreshWaitOutcome::Continue => continue,
                        RefreshWaitOutcome::Selected {
                            cards,
                            skip_post_select,
                        } => {
                            return Ok(GenerateLoopResult::Selected {
                                cards,
                                skip_post_select,
                            });
                        }
                        RefreshWaitOutcome::Outcome(outcome) => {
                            return Ok(GenerateLoopResult::EarlyOutcome(outcome));
                        }
                    }
                } else {
                    progress.step_error(PipelineStep::Validate, &format!("{e}"));
                    return Err(e);
                }
            }
        };

        // Attach pre-select flags
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
                .map(|s| strip_html_tags(s).to_lowercase())
                .unwrap_or_default();
            if key.is_empty() || seen_keys.insert(key) {
                new_cards.push(card);
            }
        }

        let dup_count = new_cards.iter().filter(|c| c.is_duplicate).count();
        progress.step_done(
            PipelineStep::Validate,
            if dup_count > 0 {
                Some(format!("{dup_count} duplicate(s)"))
            } else {
                None
            },
        );
        if dup_count > 0 {
            progress.log(&format!("Found {dup_count} duplicate(s) (already in Anki)"));
        }

        if is_refresh {
            if new_cards.is_empty() {
                progress.log("No new unique cards generated");
            } else {
                progress.log(&format!("{} new card(s) added", new_cards.len()));
            }
            interaction.append_selection(new_cards);
        } else {
            if config.dry_run {
                return Ok(GenerateLoopResult::EarlyOutcome(emit_dry_run(
                    progress, &new_cards,
                )));
            }

            if new_cards.is_empty() {
                return Ok(GenerateLoopResult::EarlyOutcome(PipelineOutcome::Success {
                    message: "No cards to select from.".to_string(),
                    cards: Vec::new(),
                    note_ids: Vec::new(),
                    failed: false,
                }));
            }

            progress.step_start(PipelineStep::Select, None);
            interaction.begin_selection(new_cards);
        }

        // Wait for user action. The TUI is the source of truth for
        // selection-phase card data from this point on; the worker
        // does not retain any mirror of the cards it just sent.
        match wait_selection_with_regen(config, interaction, progress) {
            SelectionAction::Refresh => {
                is_refresh = true;
                continue;
            }
            SelectionAction::RefreshWithTerm(t) => {
                is_refresh = true;
                current_term = t;
                continue;
            }
            SelectionAction::Selected {
                cards,
                skip_post_select,
            } => {
                return Ok(GenerateLoopResult::Selected {
                    cards,
                    skip_post_select,
                });
            }
            SelectionAction::Cancel => {
                return Ok(GenerateLoopResult::EarlyOutcome(PipelineOutcome::Cancelled));
            }
            SelectionAction::Quit => {
                return Ok(GenerateLoopResult::EarlyOutcome(PipelineOutcome::Quit));
            }
            SelectionAction::RegenerateCard { .. } | SelectionAction::PreviewTts { .. } => {
                unreachable!()
            }
        }
    }
}

/// Run the configured pre-select processing steps on `candidates` in
/// place, mutating it to the surviving candidate set. Returns the flag
/// list to be attached to the validated cards downstream.
fn run_pre_select(
    config: &PipelineConfig,
    candidates: &mut Vec<CardCandidate>,
    progress: &dyn PipelineProgress,
    on_log: &(dyn Fn(&str) + Send + Sync),
) -> Result<Vec<CardFlag>> {
    let pre_select_steps = config
        .frontmatter
        .processing
        .as_ref()
        .map(|p| p.pre_select.as_slice())
        .unwrap_or_default();

    if pre_select_steps.is_empty() || candidates.is_empty() {
        progress.step_skip(PipelineStep::PostProcess);
        return Ok(Vec::new());
    }

    progress.step_start(PipelineStep::PostProcess, None);
    let proc_result = run_processors(
        pre_select_steps,
        std::mem::take(candidates),
        config.field_map_keys,
        config.client,
        config.model,
        config.temperature,
        config.max_tokens,
        config.retries,
        Some(config.logger),
        on_log,
    )?;
    *candidates = proc_result.cards;
    if proc_result.cost > 0.0 {
        progress.cost_update(
            proc_result.input_tokens,
            proc_result.output_tokens,
            proc_result.cost,
        );
    }
    if proc_result.rejected_count > 0 {
        progress.log(&format!(
            "{} card(s) rejected by pre-select checks",
            proc_result.rejected_count
        ));
    }
    progress.step_done(PipelineStep::PostProcess, None);
    Ok(proc_result.flags)
}

enum RefreshWaitOutcome {
    Continue,
    Selected {
        cards: Vec<ValidatedCard>,
        skip_post_select: bool,
    },
    Outcome(PipelineOutcome),
}

/// After a refresh-time error, wait for the user's next selection
/// action. Updates `is_refresh` / `current_term` for the caller to
/// continue the outer loop, or returns a terminal outcome.
fn handle_refresh_wait(
    config: &PipelineConfig,
    interaction: &dyn PipelineInteraction,
    progress: &dyn PipelineProgress,
    is_refresh: &mut bool,
    current_term: &mut String,
) -> RefreshWaitOutcome {
    match wait_selection_with_regen(config, interaction, progress) {
        SelectionAction::Refresh => {
            *is_refresh = true;
            RefreshWaitOutcome::Continue
        }
        SelectionAction::RefreshWithTerm(t) => {
            *is_refresh = true;
            *current_term = t;
            RefreshWaitOutcome::Continue
        }
        SelectionAction::Selected {
            cards,
            skip_post_select,
        } => RefreshWaitOutcome::Selected {
            cards,
            skip_post_select,
        },
        SelectionAction::Cancel => RefreshWaitOutcome::Outcome(PipelineOutcome::Cancelled),
        SelectionAction::Quit => RefreshWaitOutcome::Outcome(PipelineOutcome::Quit),
        SelectionAction::RegenerateCard { .. } | SelectionAction::PreviewTts { .. } => {
            unreachable!()
        }
    }
}

/// Print the dry-run card listing and return the success outcome.
fn emit_dry_run(progress: &dyn PipelineProgress, new_cards: &[ValidatedCard]) -> PipelineOutcome {
    progress.step_skip(PipelineStep::Select);
    progress.step_skip(PipelineStep::QualityCheck);
    progress.step_skip(PipelineStep::Finish);
    for (i, card) in new_cards.iter().enumerate() {
        let dup = if card.is_duplicate {
            " (Duplicate)"
        } else {
            ""
        };
        progress.log(&format!("Card {}{dup}", i + 1));
        for (name, value) in &card.raw_anki_fields {
            progress.log(&format!("  {name}: {value}"));
        }
    }
    PipelineOutcome::Success {
        message: "Dry run complete. No cards were imported.".to_string(),
        cards: Vec::new(),
        note_ids: Vec::new(),
        failed: false,
    }
}

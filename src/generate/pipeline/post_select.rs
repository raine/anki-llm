use anyhow::Result;

use super::super::cards::{ValidatedCard, map_fields_to_anki};
use super::super::process::{CardFlag, FlaggedCard, run_processors};
use super::super::processor::CardCandidate;
use super::super::sanitize::sanitize_fields;
use super::types::{
    PipelineConfig, PipelineInteraction, PipelineOutcome, PipelineProgress, PipelineStep,
    ReviewResult,
};

pub(super) struct PostSelectInput {
    pub cards: Vec<ValidatedCard>,
    pub skip_post_select: bool,
}

pub(super) enum PostSelectResult {
    Continue(Vec<ValidatedCard>),
    Outcome(PipelineOutcome),
}

/// Filter the user-selected cards (dropping duplicates) and run the
/// configured post-select processing if any. Re-validates duplicates
/// when a transform writes to the identity field, then routes flagged
/// cards through the interaction's review hook.
pub(super) fn run_post_select(
    config: &PipelineConfig,
    interaction: &dyn PipelineInteraction,
    progress: &dyn PipelineProgress,
    input: PostSelectInput,
    on_log: &(dyn Fn(&str) + Send + Sync),
) -> Result<PostSelectResult> {
    let PostSelectInput {
        cards: selected,
        skip_post_select,
    } = input;
    let first_field_name = &config.validation.note_type_fields[0];

    if selected.is_empty() {
        return Ok(PostSelectResult::Outcome(PipelineOutcome::Success {
            message: "No cards selected.".to_string(),
            cards: Vec::new(),
            note_ids: Vec::new(),
            failed: false,
        }));
    }

    let mut selected = selected;

    progress.step_done(
        PipelineStep::Select,
        Some(format!("{} card(s) selected", selected.len())),
    );

    let dup_selected = selected.iter().filter(|c| c.is_duplicate).count();
    if dup_selected > 0 {
        progress.log(&format!(
            "Skipping {dup_selected} duplicate(s) — already exist in Anki."
        ));
        selected.retain(|c| !c.is_duplicate);
    }

    if selected.is_empty() {
        return Ok(PostSelectResult::Outcome(PipelineOutcome::Success {
            message: "No non-duplicate cards selected.".to_string(),
            cards: Vec::new(),
            note_ids: Vec::new(),
            failed: false,
        }));
    }

    let post_select_steps = config
        .frontmatter
        .processing
        .as_ref()
        .map(|p| p.post_select.as_slice())
        .unwrap_or_default();

    let mut post_errors: Vec<String> = Vec::new();
    let mut final_cards: Vec<ValidatedCard> = selected;

    if !post_select_steps.is_empty() && !skip_post_select {
        progress.step_start(PipelineStep::QualityCheck, None);

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
            config.field_map_keys,
            config.client,
            config.model,
            config.temperature,
            config.max_tokens,
            config.retries,
            Some(config.logger),
            on_log,
        )?;

        if proc_result.cost > 0.0 {
            progress.cost_update(
                proc_result.input_tokens,
                proc_result.output_tokens,
                proc_result.cost,
            );
        }

        // Check if any post-select transform writes to the identity field
        let first_field_key = config.field_map_keys.first().map(|s| s.as_str());
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

        final_cards = rebuild_validated_cards(proc_result.cards, config);

        if needs_revalidation {
            recheck_duplicates(&mut final_cards, config, first_field_name);
            final_cards.retain(|c| !c.is_duplicate);
        }

        match route_flagged_cards(final_cards, &post_flags, interaction, progress) {
            FlaggedRouting::Continue(cards) => final_cards = cards,
            FlaggedRouting::Cancelled => {
                return Ok(PostSelectResult::Outcome(PipelineOutcome::Cancelled));
            }
        }

        if post_rejected_count > 0 {
            progress.log(&format!(
                "{} card(s) rejected by post-select checks",
                post_rejected_count
            ));
        }
    } else if skip_post_select && !post_select_steps.is_empty() {
        progress.log("Skipping post-select processing (user toggled off)");
        progress.step_skip(PipelineStep::QualityCheck);
    } else {
        progress.step_skip(PipelineStep::QualityCheck);
    }

    if final_cards.is_empty() {
        let mut msg = "No cards remaining after processing.".to_string();
        if !post_errors.is_empty() {
            msg.push_str("\n\nErrors:\n");
            for e in &post_errors {
                msg.push_str(&format!("  • {e}\n"));
            }
        }
        return Ok(PostSelectResult::Outcome(PipelineOutcome::Success {
            message: msg,
            cards: Vec::new(),
            note_ids: Vec::new(),
            failed: false,
        }));
    }

    if !skip_post_select || post_select_steps.is_empty() {
        progress.step_done(PipelineStep::QualityCheck, None);
    }

    Ok(PostSelectResult::Continue(final_cards))
}

/// Rebuild `ValidatedCard`s from processor output, re-sanitizing and
/// re-mapping fields. Resets duplicate / flag state for the re-check
/// performed by `recheck_duplicates`.
fn rebuild_validated_cards(
    cards: Vec<CardCandidate>,
    config: &PipelineConfig,
) -> Vec<ValidatedCard> {
    cards
        .into_iter()
        .map(|c| {
            let sanitized = sanitize_fields(&c.fields);
            let anki_fields =
                map_fields_to_anki(&sanitized, &config.frontmatter.field_map).unwrap();
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
                map_fields_to_anki(&raw_strings, &config.frontmatter.field_map).unwrap();

            ValidatedCard {
                card_id: super::super::cards::next_card_id(),
                fields: sanitized,
                anki_fields,
                raw_anki_fields,
                is_duplicate: false,
                duplicate_note_id: None,
                duplicate_fields: None,
                flags: Vec::new(),
                model: config.model.to_string(),
            }
        })
        .collect()
}

fn recheck_duplicates(
    cards: &mut [ValidatedCard],
    config: &PipelineConfig,
    first_field_name: &str,
) {
    for card in cards {
        if let Some(val) = card
            .anki_fields
            .get(first_field_name)
            .filter(|v| !v.is_empty())
        {
            let query = super::super::cards::build_duplicate_query(
                &config.frontmatter.note_type,
                &config.frontmatter.deck,
                first_field_name,
                val,
            );
            card.is_duplicate = config
                .anki
                .find_notes(&query)
                .map(|ids| !ids.is_empty())
                .unwrap_or(false);
        }
    }
}

enum FlaggedRouting {
    Continue(Vec<ValidatedCard>),
    Cancelled,
}

fn route_flagged_cards(
    cards: Vec<ValidatedCard>,
    post_flags: &[CardFlag],
    interaction: &dyn PipelineInteraction,
    progress: &dyn PipelineProgress,
) -> FlaggedRouting {
    let mut passed = Vec::new();
    let mut flagged: Vec<FlaggedCard> = Vec::new();

    for (i, card) in cards.into_iter().enumerate() {
        let card_flags: Vec<&CardFlag> = post_flags.iter().filter(|f| f.card_index == i).collect();
        if card_flags.is_empty() {
            passed.push(card);
        } else {
            let reason = card_flags
                .iter()
                .map(|f| f.reason.as_str())
                .collect::<Vec<_>>()
                .join("; ");
            flagged.push(FlaggedCard { card, reason });
        }
    }

    if flagged.is_empty() {
        return FlaggedRouting::Continue(passed);
    }

    let flagged_count = flagged.len();
    progress.log(&format!(
        "{flagged_count} card(s) flagged by post-select check. Please review."
    ));

    let flagged_clone = flagged.clone();
    match interaction.request_review(flagged_clone) {
        ReviewResult::Reviewed(decisions) => {
            for (flagged_card, keep) in flagged.into_iter().zip(decisions.iter()) {
                if *keep {
                    passed.push(flagged_card.card);
                }
            }
            FlaggedRouting::Continue(passed)
        }
        ReviewResult::Cancel => FlaggedRouting::Cancelled,
    }
}

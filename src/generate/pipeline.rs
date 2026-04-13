use std::collections::HashSet;

use anyhow::Result;

use crate::anki::client::AnkiClient;
use crate::llm::client::LlmClient;
use crate::llm::logger::LlmLogger;
use crate::llm::pricing;
use crate::template::frontmatter::Frontmatter;

use super::anki_import::{TtsFinalize, import_cards_to_anki};
use super::cards::{ValidatedCard, map_fields_to_anki, validate_cards};
use super::exporter::export_cards;
use super::process::{CardFlag, FlaggedCard, run_processors};
use super::processor::{CardCandidate, generate_cards};
use super::sanitize::sanitize_fields;
use super::selector::strip_html_tags;
use super::validate::ValidationResult;

// ---------------------------------------------------------------------------
// Pipeline step identifiers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineStep {
    LoadPrompt,
    ValidateAnki,
    Generate,
    PostProcess,
    Validate,
    Select,
    QualityCheck,
    Finish,
}

impl PipelineStep {
    pub fn label(self) -> &'static str {
        match self {
            Self::LoadPrompt => "Load prompt",
            Self::ValidateAnki => "Validate Anki",
            Self::Generate => "Generate cards",
            Self::PostProcess => "Pre-select processing",
            Self::Validate => "Check duplicates",
            Self::Select => "Select cards",
            Self::QualityCheck => "Post-select processing",
            Self::Finish => "Import / export",
        }
    }
}

// ---------------------------------------------------------------------------
// Traits
// ---------------------------------------------------------------------------

pub trait PipelineProgress: Send + Sync {
    fn log(&self, msg: &str);
    fn step_start(&self, step: PipelineStep, detail: Option<&str>);
    fn step_done(&self, step: PipelineStep, detail: Option<String>);
    fn step_skip(&self, step: PipelineStep);
    fn step_error(&self, step: PipelineStep, detail: &str);
    fn cost_update(&self, input_tokens: u64, output_tokens: u64, cost: f64);
}

pub enum SelectionAction {
    Selected(Vec<usize>),
    Refresh,
    RefreshWithTerm(String),
    RegenerateCard { index: usize, feedback: String },
    PreviewTts { card_id: u64 },
    Cancel,
    Quit,
}

pub enum ReviewResult {
    Reviewed(Vec<bool>),
    Cancel,
}

pub trait PipelineInteraction {
    fn begin_selection(&self, cards: Vec<ValidatedCard>);
    fn append_selection(&self, cards: Vec<ValidatedCard>);
    fn replace_card(&self, index: usize, card: ValidatedCard);
    fn regen_error(&self, message: String);
    fn wait_selection(&self) -> SelectionAction;
    fn request_review(&self, flagged: Vec<FlaggedCard>) -> ReviewResult;
    /// Announce a TTS preview state transition for a given card id.
    /// Default impl is a no-op so legacy / copy mode can ignore it.
    fn tts_state(&self, _card_id: u64, _state: super::tui::events::TtsUiState) {}
}

// ---------------------------------------------------------------------------
// Config and outcome
// ---------------------------------------------------------------------------

pub struct PipelineConfig<'a> {
    pub frontmatter: &'a Frontmatter,
    pub prompt_body: &'a str,
    pub field_map_keys: &'a [String],
    pub validation: &'a ValidationResult,
    pub client: &'a LlmClient,
    pub anki: &'a AnkiClient,
    pub logger: &'a LlmLogger,
    pub model: &'a str,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u64>,
    pub retries: u32,
    pub count: u32,
    pub dry_run: bool,
    pub output: Option<&'a std::path::Path>,
    /// Optional TTS bundle. When present, `finalize_tts` runs before
    /// `add_notes` to synthesize + upload missing audio and rewrite the
    /// target field.
    pub tts: Option<&'a crate::tts::service::TtsBundle>,
}

pub enum PipelineOutcome {
    Success {
        message: String,
        cards: Vec<ValidatedCard>,
        note_ids: Vec<i64>,
    },
    Cancelled,
    Quit,
}

// ---------------------------------------------------------------------------
// Single-card regeneration
// ---------------------------------------------------------------------------

/// Regenerate a single card with user feedback. Returns the replacement card
/// or an error message.
fn regenerate_single_card(
    config: &PipelineConfig,
    card: &ValidatedCard,
    feedback: &str,
    progress: &dyn PipelineProgress,
) -> Result<ValidatedCard> {
    // Build a focused prompt with the original card + feedback
    let card_json: serde_json::Map<String, serde_json::Value> = card
        .fields
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
        .collect();
    let card_json_str = serde_json::to_string_pretty(&card_json)?;

    let field_keys: Vec<&str> = config.field_map_keys.iter().map(|s| s.as_str()).collect();
    let prompt = format!(
        "Here is a flashcard that was generated:\n\n\
         ```json\n{card_json_str}\n```\n\n\
         The user wants this card regenerated with the following feedback: {feedback}\n\n\
         Return ONLY a single JSON object (not an array) with the same field keys: {}.\n\
         Do not wrap in an array. Return only the JSON object, no other text.",
        field_keys.join(", ")
    );

    let result = config.client.chat_completion(
        config.model,
        &prompt,
        config.temperature,
        config.max_tokens,
    )?;

    if let Some(logger) = Some(config.logger) {
        logger.log(&prompt, &result.content);
    }

    if let Some(usage) = &result.usage {
        let cost = crate::llm::pricing::calculate_cost(
            config.model,
            usage.prompt_tokens,
            usage.completion_tokens,
        );
        progress.cost_update(usage.prompt_tokens, usage.completion_tokens, cost);
    }

    let content = result.content.trim();
    let obj = crate::llm::parse_json::try_parse_single_json_object(content)
        .ok_or_else(|| anyhow::anyhow!("Regenerated card is not a valid JSON object"))?;

    // Build CardCandidate fields
    let mut fields = std::collections::HashMap::new();
    for key in config.field_map_keys {
        let value = obj
            .get(key)
            .ok_or_else(|| anyhow::anyhow!("Regenerated card is missing field \"{key}\""))?;
        let coerced = match value {
            serde_json::Value::String(s) => serde_json::Value::String(s.clone()),
            serde_json::Value::Array(arr) => {
                let strings: Vec<String> = arr
                    .iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect();
                serde_json::Value::Array(
                    strings.into_iter().map(serde_json::Value::String).collect(),
                )
            }
            serde_json::Value::Number(n) => serde_json::Value::String(n.to_string()),
            serde_json::Value::Bool(b) => serde_json::Value::String(b.to_string()),
            serde_json::Value::Null => serde_json::Value::String(String::new()),
            _ => anyhow::bail!("Unexpected field type for \"{key}\""),
        };
        fields.insert(key.clone(), coerced);
    }

    // Sanitize and map
    let sanitized = sanitize_fields(&fields);
    let anki_fields = map_fields_to_anki(&sanitized, &config.frontmatter.field_map)?;

    let raw_strings: std::collections::HashMap<String, String> = fields
        .iter()
        .map(|(k, v)| {
            let s = match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            (k.clone(), s)
        })
        .collect();
    let raw_anki_fields = map_fields_to_anki(&raw_strings, &config.frontmatter.field_map)?;

    // Check duplicate
    let first_field_name = &config.validation.note_type_fields[0];
    let is_duplicate = anki_fields
        .get(first_field_name)
        .filter(|v| !v.is_empty())
        .map(|v| {
            super::cards::check_duplicate_pub(
                config.anki,
                v,
                &config.frontmatter.note_type,
                &config.frontmatter.deck,
            )
        })
        .unwrap_or(Ok(false))?;

    Ok(ValidatedCard {
        card_id: super::cards::next_card_id(),
        fields: sanitized,
        anki_fields,
        raw_anki_fields,
        is_duplicate,
        duplicate_note_id: None,
        duplicate_fields: None,
        flags: Vec::new(),
        model: String::new(),
    })
}

/// Wait for a selection action, handling inline card regeneration and
/// TTS preview requests. Returns only terminal actions (Refresh,
/// RefreshWithTerm, Selected, Cancel, Quit).
fn wait_selection_with_regen(
    config: &PipelineConfig,
    interaction: &dyn PipelineInteraction,
    progress: &dyn PipelineProgress,
    all_validated: &mut [ValidatedCard],
) -> SelectionAction {
    loop {
        match interaction.wait_selection() {
            SelectionAction::RegenerateCard { index, feedback } => {
                progress.log(&format!(
                    "Regenerating card {index} with feedback: \"{feedback}\""
                ));
                match regenerate_single_card(config, &all_validated[index], &feedback, progress) {
                    Ok(new_card) => {
                        all_validated[index] = new_card.clone();
                        interaction.replace_card(index, new_card);
                        progress.log("Card regenerated successfully");
                    }
                    Err(e) => {
                        interaction.regen_error(format!("Regeneration failed: {e}"));
                        progress.log(&format!("Regeneration failed: {e}"));
                    }
                }
                continue;
            }
            SelectionAction::PreviewTts { card_id } => {
                handle_preview_tts(config, interaction, progress, all_validated, card_id);
                continue;
            }
            other => return other,
        }
    }
}

/// Synthesize preview audio for a card. On success, caches the audio
/// to disk and emits `TtsUiState::Ready { cache_path }` so the TUI can
/// route a `PlayerCommand::Play` to its owned audio thread. On failure,
/// emits `TtsUiState::Failed` with a user-facing message.
fn handle_preview_tts(
    config: &PipelineConfig,
    interaction: &dyn PipelineInteraction,
    progress: &dyn PipelineProgress,
    all_validated: &[ValidatedCard],
    card_id: u64,
) {
    use super::tui::events::TtsUiState;

    let Some(bundle) = config.tts else {
        // No TTS configured — silently drop the request. The TUI should
        // never send this command in that case, but defend anyway.
        return;
    };
    let Some(card) = all_validated.iter().find(|c| c.card_id == card_id) else {
        // Card was replaced before the worker picked up the request.
        return;
    };

    interaction.tts_state(card_id, TtsUiState::Synthesizing);

    let prepared = match bundle
        .service
        .prepare_from_anki_fields(&card.raw_anki_fields, &config.frontmatter.field_map)
    {
        Ok(p) => p,
        Err(e) => {
            progress.log(&format!("TTS prepare failed: {e}"));
            interaction.tts_state(card_id, TtsUiState::Failed(e.to_string()));
            return;
        }
    };

    match bundle.service.ensure_cached(&prepared) {
        Ok(_) => {
            progress.log(&format!(
                "TTS ready: {} chars → {}",
                prepared.spoken_chars, prepared.filename
            ));
            interaction.tts_state(
                card_id,
                TtsUiState::Ready {
                    cache_path: prepared.cache_path,
                },
            );
        }
        Err(e) => {
            progress.log(&format!("TTS synthesis failed: {e}"));
            interaction.tts_state(card_id, TtsUiState::Failed(e.to_string()));
        }
    }
}

// ---------------------------------------------------------------------------
// Core pipeline
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_lines)]
pub fn run_pipeline_for_term(
    config: &PipelineConfig,
    interaction: &dyn PipelineInteraction,
    progress: &dyn PipelineProgress,
    term: &str,
    exclude_terms: &[String],
) -> Result<PipelineOutcome> {
    let first_field_name = &config.validation.note_type_fields[0];
    let on_log: &(dyn Fn(&str) + Send + Sync) = &|msg: &str| {
        progress.log(msg);
    };

    let mut generation_cost = 0.0;
    let mut all_validated: Vec<ValidatedCard> = Vec::new();
    let mut seen_keys: HashSet<String> = HashSet::new();
    let mut is_refresh = false;
    let mut current_term = term.to_string();

    // --- Generate / validate / select loop (supports refresh) ---

    let selected_indices = loop {
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
        );

        let gen_result = match gen_result {
            Ok(r) => r,
            Err(e) => {
                if is_refresh {
                    progress.log(&format!("Refresh failed: {e}"));
                    interaction.append_selection(Vec::new());
                    progress.step_error(PipelineStep::Generate, &format!("{e}"));

                    match wait_selection_with_regen(
                        config,
                        interaction,
                        progress,
                        &mut all_validated,
                    ) {
                        SelectionAction::Refresh => continue,
                        SelectionAction::RefreshWithTerm(t) => {
                            current_term = t;
                            continue;
                        }
                        SelectionAction::Selected(indices) => break indices,
                        SelectionAction::Cancel => return Ok(PipelineOutcome::Cancelled),
                        SelectionAction::Quit => return Ok(PipelineOutcome::Quit),
                        SelectionAction::RegenerateCard { .. }
                        | SelectionAction::PreviewTts { .. } => {
                            unreachable!()
                        }
                    }
                } else {
                    progress.step_error(PipelineStep::Generate, &format!("{e}"));
                    return Err(e);
                }
            }
        };

        if let Some(ref cost) = gen_result.cost {
            generation_cost += cost.total_cost;
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

        // Pre-select processing
        let pre_select_steps = config
            .frontmatter
            .processing
            .as_ref()
            .map(|p| p.pre_select.as_slice())
            .unwrap_or_default();

        let mut pre_select_flags: Vec<CardFlag> = Vec::new();

        if !pre_select_steps.is_empty() && !candidates.is_empty() {
            progress.step_start(PipelineStep::PostProcess, None);
            let proc_result = run_processors(
                pre_select_steps,
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
            candidates = proc_result.cards;
            pre_select_flags = proc_result.flags;
            generation_cost += proc_result.cost;
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
        } else {
            progress.step_skip(PipelineStep::PostProcess);
        }

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

                    match wait_selection_with_regen(
                        config,
                        interaction,
                        progress,
                        &mut all_validated,
                    ) {
                        SelectionAction::Refresh => continue,
                        SelectionAction::RefreshWithTerm(t) => {
                            current_term = t;
                            continue;
                        }
                        SelectionAction::Selected(indices) => break indices,
                        SelectionAction::Cancel => return Ok(PipelineOutcome::Cancelled),
                        SelectionAction::Quit => return Ok(PipelineOutcome::Quit),
                        SelectionAction::RegenerateCard { .. }
                        | SelectionAction::PreviewTts { .. } => {
                            unreachable!()
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
            interaction.append_selection(new_cards.clone());
            all_validated.extend(new_cards);
        } else {
            // Dry-run display
            if config.dry_run {
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
                return Ok(PipelineOutcome::Success {
                    message: "Dry run complete. No cards were imported.".to_string(),
                    cards: Vec::new(),
                    note_ids: Vec::new(),
                });
            }

            if new_cards.is_empty() {
                return Ok(PipelineOutcome::Success {
                    message: "No cards to select from.".to_string(),
                    cards: Vec::new(),
                    note_ids: Vec::new(),
                });
            }

            all_validated = new_cards;
            progress.step_start(PipelineStep::Select, None);
            interaction.begin_selection(all_validated.clone());
        }

        // Wait for user action
        match wait_selection_with_regen(config, interaction, progress, &mut all_validated) {
            SelectionAction::Refresh => {
                is_refresh = true;
                continue;
            }
            SelectionAction::RefreshWithTerm(t) => {
                is_refresh = true;
                current_term = t;
                continue;
            }
            SelectionAction::Selected(indices) => break indices,
            SelectionAction::Cancel => return Ok(PipelineOutcome::Cancelled),
            SelectionAction::Quit => return Ok(PipelineOutcome::Quit),
            SelectionAction::RegenerateCard { .. } | SelectionAction::PreviewTts { .. } => {
                unreachable!()
            }
        }
    };

    if selected_indices.is_empty() {
        return Ok(PipelineOutcome::Success {
            message: "No cards selected.".to_string(),
            cards: Vec::new(),
            note_ids: Vec::new(),
        });
    }

    let mut selected: Vec<ValidatedCard> = selected_indices
        .iter()
        .filter_map(|&i| all_validated.get(i).cloned())
        .collect();

    progress.step_done(
        PipelineStep::Select,
        Some(format!("{} card(s) selected", selected.len())),
    );

    // Filter out duplicates
    let dup_selected = selected.iter().filter(|c| c.is_duplicate).count();
    if dup_selected > 0 {
        progress.log(&format!(
            "Skipping {dup_selected} duplicate(s) — already exist in Anki."
        ));
        selected.retain(|c| !c.is_duplicate);
    }

    if selected.is_empty() {
        return Ok(PipelineOutcome::Success {
            message: "No non-duplicate cards selected.".to_string(),
            cards: Vec::new(),
            note_ids: Vec::new(),
        });
    }

    // Post-select processing
    let post_select_steps = config
        .frontmatter
        .processing
        .as_ref()
        .map(|p| p.post_select.as_slice())
        .unwrap_or_default();

    let mut post_select_cost = 0.0;
    let mut post_errors: Vec<String> = Vec::new();
    let mut final_cards: Vec<ValidatedCard> = selected;

    if !post_select_steps.is_empty() {
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

        post_select_cost = proc_result.cost;
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

        final_cards = proc_result
            .cards
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
                    card_id: super::cards::next_card_id(),
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
            .collect();

        // Re-check duplicates if identity field may have changed
        if needs_revalidation {
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
                        config.frontmatter.note_type, config.frontmatter.deck
                    );
                    card.is_duplicate = config
                        .anki
                        .find_notes(&query)
                        .map(|ids| !ids.is_empty())
                        .unwrap_or(false);
                }
            }
            final_cards.retain(|c| !c.is_duplicate);
        }

        // Handle flagged cards
        let mut passed = Vec::new();
        let mut flagged: Vec<FlaggedCard> = Vec::new();

        for (i, card) in final_cards.into_iter().enumerate() {
            let card_flags: Vec<&CardFlag> =
                post_flags.iter().filter(|f| f.card_index == i).collect();
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

        final_cards = passed;

        if !flagged.is_empty() {
            let flagged_count = flagged.len();
            progress.log(&format!(
                "{flagged_count} card(s) flagged by post-select check. Please review."
            ));

            let flagged_clone = flagged.clone();
            match interaction.request_review(flagged_clone) {
                ReviewResult::Reviewed(decisions) => {
                    for (flagged_card, keep) in flagged.into_iter().zip(decisions.iter()) {
                        if *keep {
                            final_cards.push(flagged_card.card);
                        }
                    }
                }
                ReviewResult::Cancel => return Ok(PipelineOutcome::Cancelled),
            }
        }

        if post_rejected_count > 0 {
            progress.log(&format!(
                "{} card(s) rejected by post-select checks",
                post_rejected_count
            ));
        }
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
        return Ok(PipelineOutcome::Success {
            message: msg,
            cards: Vec::new(),
            note_ids: Vec::new(),
        });
    }

    let total_cost = generation_cost + post_select_cost;
    if total_cost > 0.0 {
        progress.log(&format!("Total cost: {}", pricing::format_cost(total_cost)));
    }

    progress.step_done(PipelineStep::QualityCheck, Some("done".to_string()));

    // Export or import
    progress.step_start(PipelineStep::Finish, None);

    if let Some(output_path) = config.output {
        export_cards(&final_cards, output_path, on_log)?;
        progress.step_done(
            PipelineStep::Finish,
            Some(format!("exported to {}", output_path.display())),
        );
        Ok(PipelineOutcome::Success {
            message: format!(
                "Exported {} card(s) to {}",
                final_cards.len(),
                output_path.display()
            ),
            cards: final_cards,
            note_ids: Vec::new(),
        })
    } else {
        let tts_finalize = config.tts.map(|bundle| TtsFinalize {
            service: &bundle.service,
            media: &bundle.media,
        });
        let result = import_cards_to_anki(
            &final_cards,
            config.frontmatter,
            config.anki,
            tts_finalize,
            on_log,
        )?;
        let note_ids = result.note_ids.clone();

        if result.failures > 0 {
            progress.step_done(
                PipelineStep::Finish,
                Some(format!(
                    "{} added, {} failed",
                    result.successes, result.failures
                )),
            );
            Ok(PipelineOutcome::Success {
                message: format!(
                    "Import completed with errors: {} added, {} failed.",
                    result.successes, result.failures
                ),
                cards: final_cards,
                note_ids,
            })
        } else {
            progress.step_done(
                PipelineStep::Finish,
                Some(format!("{} card(s) added", result.successes)),
            );
            Ok(PipelineOutcome::Success {
                message: format!(
                    "Successfully added {} new note(s) to \"{}\"",
                    result.successes, config.frontmatter.deck
                ),
                cards: final_cards,
                note_ids,
            })
        }
    }
}

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
    /// UI-only terminal step. The pipeline never emits events for it;
    /// the TUI marks it Done when the run reaches `RunDone` and routes
    /// the run summary view to this step's sidebar entry.
    Summary,
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
            Self::Summary => "Summary",
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
    Selected(Vec<ValidatedCard>),
    Refresh,
    RefreshWithTerm(String),
    RegenerateCard {
        card: ValidatedCard,
        feedback: String,
    },
    PreviewTts {
        card: ValidatedCard,
    },
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
    fn replace_card(&self, previous_card_id: u64, card: ValidatedCard);
    fn regen_error(&self, target_id: u64, message: String);
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
    /// Optional lazy TTS handle. When present, the preview and import
    /// paths resolve the underlying `TtsBundle` via
    /// `SessionTts::bundle()` on first use; neither `--dry-run` nor
    /// `--output` touch this field, so TTS credential resolution is
    /// deferred (or skipped) for those flows.
    pub tts: Option<&'a crate::tts::service::SessionTts>,
}

pub enum PipelineOutcome {
    Success {
        message: String,
        cards: Vec<ValidatedCard>,
        note_ids: Vec<i64>,
        /// When true, the run finished with a non-fatal failure and the
        /// message should be rendered in an error style. The cards are
        /// still returned so the user can recover them (copy-to-clipboard
        /// from the Done view). Used to preserve user-curated state when
        /// late-stage steps like `finalize_tts` fail transiently.
        failed: bool,
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

    // Sanitize and hand off to the shared rebuild helper — this gives
    // us anki_fields + raw_anki_fields, the duplicate lookup, and the
    // on-duplicate `duplicate_fields` fetch, matching `validate_cards`'s
    // shape. Previously this constructor hardcoded
    // `duplicate_note_id: None`, `duplicate_fields: None`, and
    // `model: String::new()`, which silently broke the duplicate diff
    // panel for regenerated cards and dropped the model label in
    // multi-model sessions.
    let sanitized = sanitize_fields(&fields);
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
    let first_field_name = &config.validation.note_type_fields[0];
    super::cards::build_validated_card(
        sanitized,
        &raw_strings,
        config.frontmatter,
        first_field_name,
        config.anki,
        config.model,
    )
}

/// Wait for a selection action, handling inline card regeneration and
/// TTS preview requests. Returns only terminal actions (Refresh,
/// RefreshWithTerm, Selected, Cancel, Quit).
///
/// The worker holds no card state during this loop. Regeneration and
/// preview actions both carry the TUI's current `ValidatedCard` snapshot
/// in the message payload, so any local edits the user has applied are
/// reflected in what the worker operates on.
fn wait_selection_with_regen(
    config: &PipelineConfig,
    interaction: &dyn PipelineInteraction,
    progress: &dyn PipelineProgress,
) -> SelectionAction {
    loop {
        match interaction.wait_selection() {
            SelectionAction::RegenerateCard { card, feedback } => {
                let previous_card_id = card.card_id;
                progress.log(&format!(
                    "Regenerating card {previous_card_id} with feedback: \"{feedback}\""
                ));
                match regenerate_single_card(config, &card, &feedback, progress) {
                    Ok(new_card) => {
                        interaction.replace_card(previous_card_id, new_card);
                        progress.log("Card regenerated successfully");
                    }
                    Err(e) => {
                        interaction
                            .regen_error(previous_card_id, format!("Regeneration failed: {e}"));
                        progress.log(&format!("Regeneration failed: {e}"));
                    }
                }
                continue;
            }
            SelectionAction::PreviewTts { card } => {
                handle_preview_tts(config, interaction, progress, &card);
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
    card: &ValidatedCard,
) {
    use super::tui::events::TtsUiState;

    let Some(session_tts) = config.tts else {
        // No TTS configured — silently drop the request. The TUI should
        // never send this command in that case, but defend anyway.
        return;
    };
    let card_id = card.card_id;

    interaction.tts_state(card_id, TtsUiState::Synthesizing);

    // First preview in a session materializes the bundle via
    // `spec::resolve`. If credentials are missing the failure surfaces
    // here as a per-card `Failed` state, not a session-wide fatal — the
    // user can fix the env and retry without losing their curation.
    let bundle = match session_tts.bundle() {
        Ok(b) => b,
        Err(e) => {
            progress.log(&format!("TTS unavailable: {e:#}"));
            interaction.tts_state(card_id, TtsUiState::Failed(format!("{e:#}")));
            return;
        }
    };

    let prepared = match bundle
        .service
        .prepare_from_anki_fields(&card.raw_anki_fields, &config.frontmatter.field_map)
    {
        Ok(p) => p,
        Err(e) => {
            progress.log(&format!("TTS prepare failed: {e:#}"));
            interaction.tts_state(card_id, TtsUiState::Failed(format!("{e:#}")));
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
            progress.log(&format!("TTS synthesis failed: {e:#}"));
            interaction.tts_state(card_id, TtsUiState::Failed(format!("{e:#}")));
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
    let mut seen_keys: HashSet<String> = HashSet::new();
    let mut is_refresh = false;
    let mut current_term = term.to_string();

    // --- Generate / validate / select loop (supports refresh) ---
    //
    // The worker holds no card state between iterations: each batch's
    // validated cards are handed to the TUI via `begin_selection` /
    // `append_selection`, and the TUI is the source of truth for every
    // mutation (selection, edit, remove, force-toggle) until the user
    // submits. `seen_keys` is the only worker-side state that survives
    // a refresh, and it's purely a normalized-text dedup set for the
    // LLM exclude list — it never indexes into a card collection.

    let selected_cards = loop {
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

                    match wait_selection_with_regen(config, interaction, progress) {
                        SelectionAction::Refresh => continue,
                        SelectionAction::RefreshWithTerm(t) => {
                            current_term = t;
                            continue;
                        }
                        SelectionAction::Selected(cards) => break cards,
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

                    match wait_selection_with_regen(config, interaction, progress) {
                        SelectionAction::Refresh => continue,
                        SelectionAction::RefreshWithTerm(t) => {
                            current_term = t;
                            continue;
                        }
                        SelectionAction::Selected(cards) => break cards,
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
            interaction.append_selection(new_cards);
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
                    failed: false,
                });
            }

            if new_cards.is_empty() {
                return Ok(PipelineOutcome::Success {
                    message: "No cards to select from.".to_string(),
                    cards: Vec::new(),
                    note_ids: Vec::new(),
                    failed: false,
                });
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
            SelectionAction::Selected(cards) => break cards,
            SelectionAction::Cancel => return Ok(PipelineOutcome::Cancelled),
            SelectionAction::Quit => return Ok(PipelineOutcome::Quit),
            SelectionAction::RegenerateCard { .. } | SelectionAction::PreviewTts { .. } => {
                unreachable!()
            }
        }
    };

    if selected_cards.is_empty() {
        return Ok(PipelineOutcome::Success {
            message: "No cards selected.".to_string(),
            cards: Vec::new(),
            note_ids: Vec::new(),
            failed: false,
        });
    }

    let mut selected: Vec<ValidatedCard> = selected_cards;

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
            failed: false,
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
            failed: false,
        });
    }

    let total_cost = generation_cost + post_select_cost;
    if total_cost > 0.0 {
        progress.log(&format!("Total cost: {}", pricing::format_cost(total_cost)));
    }

    progress.step_done(PipelineStep::QualityCheck, None);

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
            failed: false,
        })
    } else {
        // Resolve the TTS bundle lazily for import finalization. A
        // failure here — e.g. missing `AZURE_TTS_KEY` — becomes the
        // same "Import failed" surface as any other late-stage error,
        // preserving the curated card list via
        // `PipelineOutcome::Success { failed: true, .. }` instead of
        // tearing down the selection state.
        let bundle = match config.tts {
            Some(session_tts) => match session_tts.bundle() {
                Ok(b) => Some(b),
                Err(e) => {
                    progress.step_error(PipelineStep::Finish, &format!("{e}"));
                    return Ok(PipelineOutcome::Success {
                        message: format!("Import failed: {e}"),
                        cards: final_cards,
                        note_ids: Vec::new(),
                        failed: true,
                    });
                }
            },
            None => None,
        };
        let tts_finalize = bundle.map(|b| TtsFinalize {
            service: &b.service,
            media: b.media.as_ref(),
        });
        Ok(run_import_step(
            final_cards,
            config.frontmatter,
            config.anki,
            tts_finalize,
            progress,
            on_log,
        ))
    }
}

/// Import `final_cards` into Anki (including TTS finalization when
/// requested) and convert the result into a `PipelineOutcome`. Import
/// errors — including transient TTS synth/upload failures — are
/// surfaced as `PipelineOutcome::Success { failed: true, cards, .. }`
/// so the user's curated selection state survives the Done view
/// instead of getting torn down by the TUI's `RunError` handler.
fn run_import_step(
    mut final_cards: Vec<ValidatedCard>,
    frontmatter: &Frontmatter,
    anki: &AnkiClient,
    tts: Option<TtsFinalize<'_>>,
    progress: &dyn PipelineProgress,
    on_log: &(dyn Fn(&str) + Send + Sync),
) -> PipelineOutcome {
    let result = match import_cards_to_anki(&mut final_cards, frontmatter, anki, tts, on_log) {
        Ok(result) => result,
        Err(e) => {
            progress.step_error(PipelineStep::Finish, &format!("{e}"));
            return PipelineOutcome::Success {
                message: format!("Import failed: {e}"),
                cards: final_cards,
                note_ids: Vec::new(),
                failed: true,
            };
        }
    };
    let note_ids = result.note_ids.clone();

    if result.failures > 0 {
        progress.step_done(
            PipelineStep::Finish,
            Some(format!(
                "{} added, {} failed",
                result.successes, result.failures
            )),
        );
        PipelineOutcome::Success {
            message: format!(
                "Import completed with errors: {} added, {} failed.",
                result.successes, result.failures
            ),
            cards: final_cards,
            note_ids,
            failed: false,
        }
    } else {
        progress.step_done(
            PipelineStep::Finish,
            Some(format!("{} card(s) added", result.successes)),
        );
        PipelineOutcome::Success {
            message: format!(
                "Successfully added {} new note(s) to \"{}\"",
                result.successes, frontmatter.deck
            ),
            cards: final_cards,
            note_ids,
            failed: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::template::frontmatter::{TtsSource, TtsSpec};
    use crate::tts::cache::TtsCache;
    use crate::tts::error::TtsError;
    use crate::tts::media::AnkiMediaStore;
    use crate::tts::provider::{AudioFormat, SynthesisRequest, TextFormat, TtsProvider};
    use crate::tts::service::{TtsService, TtsServiceConfig};
    use crate::tts::template::TemplateSource;
    use indexmap::IndexMap;
    use std::sync::{Arc, Mutex};

    struct FailingProvider {
        calls: Mutex<usize>,
    }

    impl TtsProvider for FailingProvider {
        fn id(&self) -> &'static str {
            "failing-mock"
        }
        fn text_format(&self) -> TextFormat {
            TextFormat::PlainText
        }
        fn synthesize(&self, _req: &SynthesisRequest) -> Result<Vec<u8>, TtsError> {
            *self.calls.lock().unwrap() += 1;
            Err(TtsError::Transient("simulated synth failure".into()))
        }
    }

    struct NoopProgress;
    impl PipelineProgress for NoopProgress {
        fn log(&self, _: &str) {}
        fn step_start(&self, _: PipelineStep, _: Option<&str>) {}
        fn step_done(&self, _: PipelineStep, _: Option<String>) {}
        fn step_skip(&self, _: PipelineStep) {}
        fn step_error(&self, _: PipelineStep, _: &str) {}
        fn cost_update(&self, _: u64, _: u64, _: f64) {}
    }

    fn mk_frontmatter() -> Frontmatter {
        let mut field_map = IndexMap::new();
        field_map.insert("front".to_string(), "Front".to_string());
        field_map.insert("back".to_string(), "Back".to_string());
        Frontmatter {
            title: None,
            description: None,
            deck: "Test".into(),
            note_type: "Basic".into(),
            field_map,
            processing: None,
            tts: Some(TtsSpec {
                target: "Audio".into(),
                source: TtsSource {
                    field: Some("front".into()),
                    template: None,
                },
                voice: "alloy".into(),
                provider: None,
                region: None,
                model: None,
                format: None,
                speed: None,
            }),
        }
    }

    fn mk_card(front: &str) -> ValidatedCard {
        use std::collections::HashMap;
        let mut fields: HashMap<String, String> = HashMap::new();
        fields.insert("front".into(), front.to_string());
        let mut anki_fields: IndexMap<String, String> = IndexMap::new();
        anki_fields.insert("Front".into(), front.to_string());
        anki_fields.insert("Back".into(), "gloss".into());
        anki_fields.insert("Audio".into(), String::new());
        let raw_anki_fields = anki_fields.clone();
        ValidatedCard {
            card_id: crate::generate::cards::next_card_id(),
            fields,
            anki_fields,
            raw_anki_fields,
            is_duplicate: false,
            duplicate_note_id: None,
            duplicate_fields: None,
            flags: Vec::new(),
            model: "test".into(),
        }
    }

    /// When TTS finalization fails during import, the pipeline must
    /// preserve the curated cards via `PipelineOutcome::Success {
    /// failed: true, cards, .. }` rather than propagating the error.
    /// Otherwise the TUI's `RunError` path tears down the selection
    /// state and the user loses all of their manual curation.
    #[test]
    fn import_step_preserves_cards_when_finalize_tts_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = Arc::new(TtsCache::new(tmp.path().to_path_buf()).unwrap());
        let failing = Arc::new(FailingProvider {
            calls: Mutex::new(0),
        });
        let provider: Arc<dyn TtsProvider> = failing.clone();
        let service = TtsService::new(TtsServiceConfig {
            provider,
            cache,
            source: Arc::new(TemplateSource::field("front".into())),
            target_field: "Audio".into(),
            voice: "alloy".into(),
            model: None,
            format: AudioFormat::Mp3,
            speed: None,
            endpoint: None,
        });
        let media = AnkiMediaStore::new(AnkiClient::new());
        let finalizer = TtsFinalize {
            service: &service,
            media: &media,
        };

        let frontmatter = mk_frontmatter();
        let anki = AnkiClient::new();
        let cards = vec![mk_card("alpha"), mk_card("beta"), mk_card("gamma")];
        let progress = NoopProgress;

        let outcome = run_import_step(
            cards,
            &frontmatter,
            &anki,
            Some(finalizer),
            &progress,
            &|_| {},
        );

        assert!(
            *failing.calls.lock().unwrap() >= 1,
            "provider should have been called at least once before failing"
        );
        match outcome {
            PipelineOutcome::Success {
                failed: true,
                cards,
                note_ids,
                message,
            } => {
                assert_eq!(cards.len(), 3, "curated cards must survive the failure");
                assert!(note_ids.is_empty(), "nothing should be imported");
                assert!(
                    message.starts_with("Import failed:"),
                    "message should surface the error, got: {message}"
                );
            }
            other => {
                let _ = other;
                panic!("expected Success with failed=true, got other outcome");
            }
        }
    }
}

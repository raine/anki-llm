use anyhow::Result;

use super::super::cards::ValidatedCard;
use super::super::sanitize::sanitize_fields;
use super::preview::handle_preview_tts;
use super::types::{PipelineConfig, PipelineInteraction, PipelineProgress, SelectionAction};

/// Regenerate a single card with user feedback. Returns the replacement card
/// or an error message.
fn regenerate_single_card(
    config: &PipelineConfig,
    card: &ValidatedCard,
    feedback: &str,
    progress: &dyn PipelineProgress,
) -> Result<ValidatedCard> {
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
    super::super::cards::build_validated_card(
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
pub(super) fn wait_selection_with_regen(
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

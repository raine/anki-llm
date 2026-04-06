use std::collections::HashMap;

use serde_json::Value;

use crate::data::Row;
use crate::llm::client::LlmClient;
use crate::llm::error::LlmError;
use crate::llm::logger::LlmLogger;
use crate::llm::parse_json::try_parse_json_array;
use crate::llm::pricing;
use crate::llm::retry::retry_with_backoff;
use crate::template::fill_template;

/// A single generated card candidate.
pub struct CardCandidate {
    /// Fields as returned by the LLM (using LLM keys, not Anki field names).
    /// Values can be String or Array (for list fields).
    pub fields: HashMap<String, Value>,
}

pub struct CostInfo {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_cost: f64,
}

pub struct GenerationResult {
    pub cards: Vec<CardCandidate>,
    pub cost: Option<CostInfo>,
}

/// Generate card candidates by making a single LLM call.
///
/// The prompt template must contain `{term}` and `{count}` placeholders.
/// The response is expected to be a JSON array of objects with keys matching
/// the fieldMap keys.
#[allow(clippy::too_many_arguments)]
pub fn generate_cards(
    term: &str,
    prompt_template: &str,
    count: u32,
    field_map_keys: &[String],
    client: &LlmClient,
    model: &str,
    temperature: Option<f64>,
    max_tokens: Option<u64>,
    retries: u32,
    logger: Option<&LlmLogger>,
) -> Result<GenerationResult, anyhow::Error> {
    // Build row for template filling
    let mut row = Row::new();
    row.insert("term".into(), Value::String(term.into()));
    row.insert("count".into(), Value::String(count.to_string()));

    let filled_prompt = fill_template(prompt_template, &row)?;

    retry_with_backoff(
        retries,
        |e: &anyhow::Error| matches!(e.downcast_ref::<LlmError>(), Some(LlmError::Api(_))),
        || {
            try_generate(
                &filled_prompt,
                field_map_keys,
                client,
                model,
                temperature,
                max_tokens,
                logger,
            )
        },
    )
    .map_err(|e| anyhow::anyhow!("Generation failed after retries: {e}"))
}

fn try_generate(
    prompt: &str,
    field_map_keys: &[String],
    client: &LlmClient,
    model: &str,
    temperature: Option<f64>,
    max_tokens: Option<u64>,
    logger: Option<&LlmLogger>,
) -> Result<GenerationResult, anyhow::Error> {
    let result = client.chat_completion(model, prompt, temperature, max_tokens)?;

    if let Some(logger) = logger {
        logger.log(prompt, &result.content);
    }

    let content = result.content.trim().to_string();
    if content.is_empty() {
        anyhow::bail!("Empty response from LLM");
    }

    // Parse as JSON array
    let parsed = try_parse_json_array(&content)
        .ok_or_else(|| anyhow::anyhow!("LLM response is not a valid JSON array"))?;

    // Validate and convert each card, skipping malformed ones
    let mut cards = Vec::new();
    for (i, obj) in parsed.into_iter().enumerate() {
        let mut fields = HashMap::new();
        let mut malformed = false;
        for key in field_map_keys {
            let Some(value) = obj.get(key) else {
                eprintln!(
                    "  Warning: Card {} is missing field \"{key}\". Skipping.",
                    i + 1
                );
                malformed = true;
                break;
            };
            // Coerce numbers and booleans to strings
            let coerced = match value {
                Value::String(s) => Value::String(s.clone()),
                Value::Array(arr) => {
                    let strings: Vec<String> = arr
                        .iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect();
                    Value::Array(strings.into_iter().map(Value::String).collect())
                }
                Value::Number(n) => Value::String(n.to_string()),
                Value::Bool(b) => Value::String(b.to_string()),
                Value::Null => Value::String(String::new()),
                _ => {
                    eprintln!(
                        "  Warning: Card {} has unsupported field type for \"{key}\". Skipping.",
                        i + 1
                    );
                    malformed = true;
                    break;
                }
            };
            fields.insert(key.clone(), coerced);
        }
        if !malformed {
            cards.push(CardCandidate { fields });
        }
    }

    let cost = result.usage.map(|u| {
        let total = pricing::calculate_cost(model, u.prompt_tokens, u.completion_tokens);
        CostInfo {
            input_tokens: u.prompt_tokens,
            output_tokens: u.completion_tokens,
            total_cost: total,
        }
    });

    Ok(GenerationResult { cards, cost })
}

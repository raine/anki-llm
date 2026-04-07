use std::collections::HashMap;
use std::time::Duration;

use serde_json::Value;

use crate::data::Row;
use crate::llm::client::LlmClient;
use crate::llm::error::LlmError;
use crate::llm::logger::LlmLogger;
use crate::llm::parse_json::try_parse_json_array;
use crate::llm::pricing;
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
    exclude_terms: Option<&[String]>,
    client: &LlmClient,
    model: &str,
    temperature: Option<f64>,
    max_tokens: Option<u64>,
    retries: u32,
    logger: Option<&LlmLogger>,
    on_log: &(dyn Fn(&str) + Send + Sync),
) -> Result<GenerationResult, anyhow::Error> {
    // Build row for template filling
    let mut row = Row::new();
    row.insert("term".into(), Value::String(term.into()));
    row.insert("count".into(), Value::String(count.to_string()));

    let mut filled_prompt = fill_template(prompt_template, &row)?;

    // Append exclusion context so the LLM avoids repeating previous cards
    if let Some(terms) = exclude_terms
        && !terms.is_empty()
    {
        filled_prompt.push_str(
            "\n\nDo not generate cards that cover the same content as these existing cards:\n",
        );
        for t in terms {
            filled_prompt.push_str(&format!("- {t}\n"));
        }
    }

    // Retry loop
    let mut last_error = String::new();
    for attempt in 0..=retries {
        if attempt > 0 {
            let backoff = Duration::from_millis(1000 * 2u64.pow(attempt - 1));
            let backoff = backoff.min(Duration::from_secs(30));
            on_log(&format!("  Retry {attempt}/{retries}: {last_error}"));
            std::thread::sleep(backoff);
        }

        match try_generate(
            &filled_prompt,
            field_map_keys,
            client,
            model,
            temperature,
            max_tokens,
            logger,
            on_log,
        ) {
            Ok(result) => return Ok(result),
            Err(e) => {
                last_error = e.to_string();
                // API errors (non-retryable) — break immediately
                if let Some(LlmError::Api(_)) = e.downcast_ref::<LlmError>() {
                    break;
                }
            }
        }
    }

    anyhow::bail!("Generation failed after retries: {last_error}")
}

#[allow(clippy::too_many_arguments)]
fn try_generate(
    prompt: &str,
    field_map_keys: &[String],
    client: &LlmClient,
    model: &str,
    temperature: Option<f64>,
    max_tokens: Option<u64>,
    logger: Option<&LlmLogger>,
    on_log: &(dyn Fn(&str) + Send + Sync),
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
    for obj in parsed {
        let mut fields = HashMap::new();
        let mut malformed = false;
        for key in field_map_keys {
            let Some(value) = obj.get(key) else {
                on_log(&format!(
                    "  Warning: card is missing field \"{key}\". Skipping."
                ));
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

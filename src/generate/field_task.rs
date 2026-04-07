use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;

use crate::data::Row;
use crate::llm::client::LlmClient;
use crate::llm::error::LlmError;
use crate::llm::logger::LlmLogger;
use crate::llm::parse_json::try_parse_json_object;
use crate::llm::pricing;
use crate::template::fill_template;
use crate::template::frontmatter::FieldTaskConfig;

use super::processor::CardCandidate;

/// How many field-task LLM calls to run in parallel.
const DEFAULT_CONCURRENCY: usize = 10;

pub struct FieldTaskResult {
    pub cards: Vec<CardCandidate>,
    pub cost: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Run field tasks on generated cards. Each task rewrites a single field using
/// a focused LLM call. Tasks run in order; within each task, cards are processed
/// concurrently. Cards that fail a task are discarded.
#[allow(clippy::too_many_arguments)]
pub fn run_field_tasks(
    cards: Vec<CardCandidate>,
    tasks: &[FieldTaskConfig],
    client: &LlmClient,
    default_model: &str,
    temperature: Option<f64>,
    max_tokens: Option<u64>,
    retries: u32,
    logger: Option<&LlmLogger>,
    on_progress: &(dyn Fn(&str) + Send + Sync),
) -> Result<FieldTaskResult, anyhow::Error> {
    let mut current_cards = cards;
    let mut total_cost = 0.0;
    let mut total_input_tokens = 0u64;
    let mut total_output_tokens = 0u64;

    for (task_idx, task) in tasks.iter().enumerate() {
        let model = task.model.as_deref().unwrap_or(default_model);
        let total_cards = current_cards.len();

        on_progress(&format!(
            "Field task {}/{}: rewriting '{}' on {} card(s) using {model}...",
            task_idx + 1,
            tasks.len(),
            task.field,
            total_cards
        ));

        // Build prompts for each card
        let mut prompts: Vec<String> = Vec::with_capacity(total_cards);
        for card in &current_cards {
            let mut row = Row::new();
            for (key, value) in &card.fields {
                let text = match value {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                row.insert(key.clone(), Value::String(text));
            }
            let filled = fill_template(&task.prompt, &row)?;
            prompts.push(filled);
        }

        // Process cards concurrently
        type CardResult = Result<(String, u64, u64, f64), String>;
        let results: Arc<Mutex<Vec<(usize, CardResult)>>> =
            Arc::new(Mutex::new(Vec::with_capacity(total_cards)));

        let next_index = Arc::new(AtomicUsize::new(0));
        let concurrency = DEFAULT_CONCURRENCY.min(total_cards);

        std::thread::scope(|s| {
            for _ in 0..concurrency {
                let next_index = Arc::clone(&next_index);
                let results = Arc::clone(&results);
                let prompts = &prompts;

                s.spawn(move || {
                    loop {
                        let idx = next_index.fetch_add(1, Ordering::SeqCst);
                        if idx >= total_cards {
                            break;
                        }

                        let outcome = run_single_task(
                            client,
                            model,
                            &prompts[idx],
                            temperature,
                            max_tokens,
                            retries,
                            logger,
                        )
                        .map_err(|e| e.to_string());

                        results.lock().unwrap().push((idx, outcome));
                    }
                });
            }
        });

        let mut results = Arc::try_unwrap(results).unwrap().into_inner().unwrap();
        results.sort_by_key(|(idx, _)| *idx);

        // Apply results — replace field or discard card
        let mut next_cards = Vec::with_capacity(total_cards);
        let mut cards_iter: Vec<Option<CardCandidate>> =
            current_cards.into_iter().map(Some).collect();

        for (idx, result) in results {
            let card = cards_iter[idx].take().unwrap();
            match result {
                Ok((value, in_tok, out_tok, cost)) => {
                    total_cost += cost;
                    total_input_tokens += in_tok;
                    total_output_tokens += out_tok;

                    let mut card = card;
                    card.fields.insert(task.field.clone(), Value::String(value));
                    next_cards.push(card);
                }
                Err(e) => {
                    on_progress(&format!(
                        "  Field task failed for card {}: {e}. Discarding.",
                        idx + 1
                    ));
                }
            }
        }

        current_cards = next_cards;
    }

    if total_cost > 0.0 {
        on_progress(&format!(
            "  Field task cost: {}",
            pricing::format_cost(total_cost)
        ));
    }

    Ok(FieldTaskResult {
        cards: current_cards,
        cost: total_cost,
        input_tokens: total_input_tokens,
        output_tokens: total_output_tokens,
    })
}

/// Run a single field task LLM call. Returns (value, input_tokens, output_tokens, cost).
fn run_single_task(
    client: &LlmClient,
    model: &str,
    prompt: &str,
    temperature: Option<f64>,
    max_tokens: Option<u64>,
    retries: u32,
    logger: Option<&LlmLogger>,
) -> Result<(String, u64, u64, f64), anyhow::Error> {
    let mut last_error = String::new();

    for attempt in 0..=retries {
        if attempt > 0 {
            let backoff = Duration::from_millis(1000 * 2u64.pow(attempt - 1));
            std::thread::sleep(backoff.min(Duration::from_secs(30)));
        }

        match client.chat_completion(model, prompt, temperature, max_tokens) {
            Ok(result) => {
                if let Some(logger) = logger {
                    logger.log(prompt, &result.content);
                }

                let value = extract_value(&result.content)?;

                let (in_tok, out_tok, cost) = result
                    .usage
                    .map(|u| {
                        (
                            u.prompt_tokens,
                            u.completion_tokens,
                            pricing::calculate_cost(model, u.prompt_tokens, u.completion_tokens),
                        )
                    })
                    .unwrap_or((0, 0, 0.0));

                return Ok((value, in_tok, out_tok, cost));
            }
            Err(e) => {
                last_error = e.to_string();
                if let LlmError::Api(_) = e {
                    break;
                }
            }
        }
    }

    anyhow::bail!("Field task failed: {last_error}")
}

/// Extract the value from the LLM response. Accepts either:
/// - JSON object with a "value" key: `{"value": "..."}`
/// - Plain text (fallback): the trimmed response content
fn extract_value(content: &str) -> Result<String, anyhow::Error> {
    let trimmed = content.trim();

    // Try JSON first
    if let Some(obj) = try_parse_json_object(trimmed)
        && let Some(Value::String(v)) = obj.get("value")
    {
        return Ok(v.clone());
    }

    // Fall back to plain text
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_json_value() {
        let content = r#"{"value": "何[なに]か あったら"}"#;
        assert_eq!(extract_value(content).unwrap(), "何[なに]か あったら");
    }

    #[test]
    fn extract_plain_text_fallback() {
        let content = "何[なに]か あったら";
        assert_eq!(extract_value(content).unwrap(), "何[なに]か あったら");
    }

    #[test]
    fn extract_json_with_whitespace() {
        let content = r#"  {"value": "test"}  "#;
        assert_eq!(extract_value(content).unwrap(), "test");
    }
}

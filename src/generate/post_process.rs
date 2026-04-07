use std::collections::HashMap;
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
use crate::template::frontmatter::PostProcessConfig;

use super::processor::CardCandidate;

/// How many post-process LLM calls to run in parallel.
const DEFAULT_CONCURRENCY: usize = 10;

pub struct PostProcessResult {
    pub cards: Vec<CardCandidate>,
    pub cost: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Run post-processing tasks on generated cards. Each task uses a focused LLM
/// call to rewrite card fields. Tasks run in order; within each task, cards are
/// processed concurrently. Cards that fail a task are discarded.
///
/// If a task has `target` set, the response is treated as a single field value
/// (plain text or `{"value": "..."}`). If `target` is omitted, the response
/// must be a JSON object whose keys are merged into the card as a partial patch.
#[allow(clippy::too_many_arguments)]
pub fn run_post_process(
    cards: Vec<CardCandidate>,
    tasks: &[PostProcessConfig],
    field_map_keys: &[String],
    client: &LlmClient,
    default_model: &str,
    temperature: Option<f64>,
    max_tokens: Option<u64>,
    retries: u32,
    logger: Option<&LlmLogger>,
    on_progress: &(dyn Fn(&str) + Send + Sync),
) -> Result<PostProcessResult, anyhow::Error> {
    let mut current_cards = cards;
    let mut total_cost = 0.0;
    let mut total_input_tokens = 0u64;
    let mut total_output_tokens = 0u64;

    for (task_idx, task) in tasks.iter().enumerate() {
        let model = task.model.as_deref().unwrap_or(default_model);
        let total_cards = current_cards.len();

        let desc = if let Some(ref target) = task.target {
            format!("rewriting '{target}'")
        } else {
            "updating fields".to_string()
        };
        on_progress(&format!(
            "Post-process {}/{}: {desc} on {total_cards} card(s) using {model}...",
            task_idx + 1,
            tasks.len(),
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
        type CardResult = Result<(HashMap<String, String>, u64, u64, f64), String>;
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

                        let outcome = run_single(
                            client,
                            model,
                            &prompts[idx],
                            task.target.as_deref(),
                            field_map_keys,
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

        // Apply results — patch fields or discard card
        let mut next_cards = Vec::with_capacity(total_cards);
        let mut cards_iter: Vec<Option<CardCandidate>> =
            current_cards.into_iter().map(Some).collect();

        for (idx, result) in results {
            let card = cards_iter[idx].take().unwrap();
            match result {
                Ok((updates, in_tok, out_tok, cost)) => {
                    total_cost += cost;
                    total_input_tokens += in_tok;
                    total_output_tokens += out_tok;

                    let mut card = card;
                    for (key, value) in updates {
                        card.fields.insert(key, Value::String(value));
                    }
                    next_cards.push(card);
                }
                Err(e) => {
                    on_progress(&format!(
                        "  Post-process failed for card {}: {e}. Discarding.",
                        idx + 1
                    ));
                }
            }
        }

        current_cards = next_cards;
    }

    if total_cost > 0.0 {
        on_progress(&format!(
            "  Post-process cost: {}",
            pricing::format_cost(total_cost)
        ));
    }

    Ok(PostProcessResult {
        cards: current_cards,
        cost: total_cost,
        input_tokens: total_input_tokens,
        output_tokens: total_output_tokens,
    })
}

/// Run a single post-process LLM call.
/// Returns (field_updates, input_tokens, output_tokens, cost).
#[allow(clippy::too_many_arguments)]
fn run_single(
    client: &LlmClient,
    model: &str,
    prompt: &str,
    target: Option<&str>,
    field_map_keys: &[String],
    temperature: Option<f64>,
    max_tokens: Option<u64>,
    retries: u32,
    logger: Option<&LlmLogger>,
) -> Result<(HashMap<String, String>, u64, u64, f64), anyhow::Error> {
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

                let updates = extract_updates(&result.content, target, field_map_keys)?;

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

                return Ok((updates, in_tok, out_tok, cost));
            }
            Err(e) => {
                last_error = e.to_string();
                if let LlmError::Api(_) = e {
                    break;
                }
            }
        }
    }

    anyhow::bail!("Post-process failed: {last_error}")
}

/// Extract field updates from the LLM response.
///
/// - With `target`: single-field mode. Accepts `{"value": "..."}` or plain text.
/// - Without `target`: multi-field mode. Expects a JSON object whose keys must
///   all exist in `field_map_keys`. Missing keys are fine (partial patch), but
///   unknown keys are rejected.
fn extract_updates(
    content: &str,
    target: Option<&str>,
    field_map_keys: &[String],
) -> Result<HashMap<String, String>, anyhow::Error> {
    let trimmed = content.trim();
    let mut updates = HashMap::new();

    if let Some(target_field) = target {
        // Single-field mode
        if let Some(obj) = try_parse_json_object(trimmed)
            && let Some(Value::String(v)) = obj.get("value")
        {
            updates.insert(target_field.to_string(), v.clone());
        } else {
            // Plain text fallback
            updates.insert(target_field.to_string(), trimmed.to_string());
        }
    } else {
        // Multi-field mode — must be a JSON object
        let obj = try_parse_json_object(trimmed).ok_or_else(|| {
            anyhow::anyhow!("Post-process expected JSON object but got plain text")
        })?;

        for (key, value) in &obj {
            if !field_map_keys.contains(key) {
                anyhow::bail!("Post-process returned unknown field '{key}'");
            }
            let text = match value {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            updates.insert(key.clone(), text);
        }

        if updates.is_empty() {
            anyhow::bail!("Post-process returned empty JSON object");
        }
    }

    Ok(updates)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> Vec<String> {
        vec![
            "front".into(),
            "kanji".into(),
            "read".into(),
            "context".into(),
        ]
    }

    #[test]
    fn single_target_json_value() {
        let content = r#"{"value": "何[なに]か あったら"}"#;
        let updates = extract_updates(content, Some("read"), &keys()).unwrap();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates["read"], "何[なに]か あったら");
    }

    #[test]
    fn single_target_plain_text() {
        let content = "何[なに]か あったら";
        let updates = extract_updates(content, Some("read"), &keys()).unwrap();
        assert_eq!(updates["read"], "何[なに]か あったら");
    }

    #[test]
    fn multi_field_json() {
        let content = r#"{"read": "何[なに]か", "context": "Casual"}"#;
        let updates = extract_updates(content, None, &keys()).unwrap();
        assert_eq!(updates.len(), 2);
        assert_eq!(updates["read"], "何[なに]か");
        assert_eq!(updates["context"], "Casual");
    }

    #[test]
    fn multi_field_partial_patch() {
        let content = r#"{"read": "test"}"#;
        let updates = extract_updates(content, None, &keys()).unwrap();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates["read"], "test");
    }

    #[test]
    fn multi_field_unknown_key_rejected() {
        let content = r#"{"read": "test", "bogus": "bad"}"#;
        assert!(extract_updates(content, None, &keys()).is_err());
    }

    #[test]
    fn multi_field_plain_text_rejected() {
        let content = "just some text";
        assert!(extract_updates(content, None, &keys()).is_err());
    }

    #[test]
    fn single_target_with_whitespace() {
        let content = r#"  {"value": "test"}  "#;
        let updates = extract_updates(content, Some("read"), &keys()).unwrap();
        assert_eq!(updates["read"], "test");
    }
}

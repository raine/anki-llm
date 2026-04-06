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
use crate::style::style;
use crate::template::fill_template;
use crate::template::frontmatter::QualityCheckConfig;

use super::cards::ValidatedCard;

/// How many QC LLM calls to run in parallel.
const DEFAULT_CONCURRENCY: usize = 10;

pub struct QualityCheckResult {
    pub final_cards: Vec<ValidatedCard>,
    pub cost: f64,
}

/// Perform quality check on selected cards. Returns filtered cards and cost.
/// If check_config is None, returns all cards unchanged with zero cost.
#[allow(clippy::too_many_arguments)]
pub fn perform_quality_check(
    cards: Vec<ValidatedCard>,
    check_config: Option<&QualityCheckConfig>,
    client: &LlmClient,
    model: &str,
    temperature: Option<f64>,
    max_tokens: Option<u64>,
    retries: u32,
    logger: Option<&LlmLogger>,
) -> Result<QualityCheckResult, anyhow::Error> {
    let config = match check_config {
        Some(c) => c,
        None => {
            return Ok(QualityCheckResult {
                final_cards: cards,
                cost: 0.0,
            });
        }
    };

    let qc_model = config.model.as_deref().unwrap_or(model);
    let total_cards = cards.len();

    // Pre-compute prompts; fail early if a field is missing or template is invalid.
    let mut cards_opt: Vec<Option<ValidatedCard>> = Vec::with_capacity(total_cards);
    let mut prompts: Vec<String> = Vec::with_capacity(total_cards);

    for card in cards {
        let text = card
            .fields
            .get(&config.field)
            .ok_or_else(|| anyhow::anyhow!("Field \"{}\" not found on card", config.field))?;

        let mut row = Row::new();
        row.insert("text".into(), Value::String(text.clone()));
        let filled_prompt = fill_template(&config.prompt, &row)
            .map_err(|e| anyhow::anyhow!("Invalid quality check prompt template: {e}"))?;

        cards_opt.push(Some(card));
        prompts.push(filled_prompt);
    }

    let spinner =
        crate::spinner::llm_spinner(format!("Quality check 0/{total_cards} using {qc_model}..."));

    // Each entry: (original_index, Ok((is_valid, reason, cost)) | Err(message))
    type CardResult = Result<(bool, String, f64), String>;
    let results: Arc<Mutex<Vec<(usize, CardResult)>>> =
        Arc::new(Mutex::new(Vec::with_capacity(total_cards)));

    let next_index = Arc::new(AtomicUsize::new(0));
    let done_count = Arc::new(AtomicUsize::new(0));

    let concurrency = DEFAULT_CONCURRENCY.min(total_cards);

    std::thread::scope(|s| {
        for _ in 0..concurrency {
            let next_index = Arc::clone(&next_index);
            let done_count = Arc::clone(&done_count);
            let results = Arc::clone(&results);
            let spinner = &spinner;
            let prompts = &prompts;

            s.spawn(move || {
                loop {
                    let idx = next_index.fetch_add(1, Ordering::SeqCst);
                    if idx >= total_cards {
                        break;
                    }

                    let outcome = check_single(
                        client,
                        qc_model,
                        &prompts[idx],
                        temperature,
                        max_tokens,
                        retries,
                        logger,
                    )
                    .map_err(|e| e.to_string());

                    let done = done_count.fetch_add(1, Ordering::SeqCst) + 1;
                    spinner.set_message(format!(
                        "Quality check {done}/{total_cards} using {qc_model}..."
                    ));

                    results.lock().unwrap().push((idx, outcome));
                }
            });
        }
    });

    spinner.finish_and_clear();

    // Sort by original card index so flagged review is in input order.
    let mut results = Arc::try_unwrap(results).unwrap().into_inner().unwrap();
    results.sort_by_key(|(idx, _)| *idx);

    let mut total_cost = 0.0;
    let mut valid_cards: Vec<ValidatedCard> = Vec::new();
    let mut flagged: Vec<(ValidatedCard, String)> = Vec::new();

    for (idx, result) in results {
        let card = cards_opt[idx].take().unwrap();
        match result {
            Ok((true, _, cost)) => {
                total_cost += cost;
                valid_cards.push(card);
            }
            Ok((false, reason, cost)) => {
                total_cost += cost;
                flagged.push((card, reason));
            }
            Err(e) => {
                let s = style();
                eprintln!(
                    "  {}",
                    s.warning(format!("Quality check failed: {e}. Discarding card."))
                );
            }
        }
    }

    let s = style();

    if total_cost > 0.0 {
        eprintln!(
            "  {}  {}",
            s.muted("Quality check cost"),
            s.muted(pricing::format_cost(total_cost))
        );
    }

    if flagged.is_empty() {
        eprintln!("  {}", s.success("All cards passed the quality check."));
        return Ok(QualityCheckResult {
            final_cards: valid_cards,
            cost: total_cost,
        });
    }

    let flagged_count = flagged.len();
    eprintln!(
        "\n  {} card(s) flagged — please review:",
        s.yellow(flagged_count)
    );

    for (i, (card, reason)) in flagged.into_iter().enumerate() {
        eprintln!("\n  {} {}/{}", s.bold("Flagged Card"), i + 1, flagged_count);
        for (key, value) in &card.fields {
            eprintln!("  {}: {}", s.muted(key), value);
        }
        eprintln!("\n  {}: {}", s.muted("Reason"), reason);

        let keep = inquire::Confirm::new("Keep this card anyway?")
            .with_default(false)
            .prompt()
            .unwrap_or(false);

        if keep {
            valid_cards.push(card);
        }
    }

    Ok(QualityCheckResult {
        final_cards: valid_cards,
        cost: total_cost,
    })
}

fn check_single(
    client: &LlmClient,
    model: &str,
    prompt: &str,
    temperature: Option<f64>,
    max_tokens: Option<u64>,
    retries: u32,
    logger: Option<&LlmLogger>,
) -> Result<(bool, String, f64), anyhow::Error> {
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

                let obj = try_parse_json_object(&result.content)
                    .ok_or_else(|| anyhow::anyhow!("Quality check response is not valid JSON"))?;

                let is_valid = obj
                    .get("is_valid")
                    .and_then(|v| v.as_bool())
                    .ok_or_else(|| anyhow::anyhow!("Missing is_valid in quality check response"))?;
                let reason = obj
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("No reason provided")
                    .to_string();

                let cost = result
                    .usage
                    .map(|u| pricing::calculate_cost(model, u.prompt_tokens, u.completion_tokens))
                    .unwrap_or(0.0);

                return Ok((is_valid, reason, cost));
            }
            Err(e) => {
                last_error = e.to_string();
                if let LlmError::Api(_) = e {
                    break;
                }
            }
        }
    }

    anyhow::bail!("Quality check failed: {last_error}")
}

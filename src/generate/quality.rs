use std::time::Duration;

use serde_json::Value;

use crate::data::Row;
use crate::llm::client::LlmClient;
use crate::llm::error::LlmError;
use crate::llm::parse_json::try_parse_json_object;
use crate::llm::pricing;
use crate::template::fill_template;
use crate::template::frontmatter::QualityCheckConfig;

use super::cards::ValidatedCard;

pub struct QualityCheckResult {
    pub final_cards: Vec<ValidatedCard>,
    pub cost: f64,
}

/// Perform quality check on selected cards. Returns filtered cards and cost.
/// If check_config is None, returns all cards unchanged with zero cost.
pub fn perform_quality_check(
    cards: Vec<ValidatedCard>,
    check_config: Option<&QualityCheckConfig>,
    client: &LlmClient,
    model: &str,
    temperature: Option<f64>,
    max_tokens: Option<u64>,
    retries: u32,
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
    let spinner =
        crate::spinner::llm_spinner(format!("Quality check 1/{total_cards} using {qc_model}..."));

    let mut total_cost = 0.0;
    let mut valid_cards = Vec::new();
    let mut flagged: Vec<(ValidatedCard, String)> = Vec::new();

    for (card_idx, card) in cards.into_iter().enumerate() {
        spinner.set_message(format!(
            "Quality check {}/{total_cards} using {qc_model}...",
            card_idx + 1
        ));
        let text = card
            .fields
            .get(&config.field)
            .ok_or_else(|| anyhow::anyhow!("Field \"{}\" not found on card", config.field))?;

        // Use fill_template for consistent template engine
        let mut row = Row::new();
        row.insert("text".into(), Value::String(text.clone()));
        let filled_prompt = fill_template(&config.prompt, &row)
            .map_err(|e| anyhow::anyhow!("Invalid quality check prompt template: {e}"))?;

        match check_single(
            client,
            qc_model,
            &filled_prompt,
            temperature,
            max_tokens,
            retries,
        ) {
            Ok((is_valid, reason, cost)) => {
                total_cost += cost;
                if is_valid {
                    valid_cards.push(card);
                } else {
                    flagged.push((card, reason));
                }
            }
            Err(e) => {
                spinner.println(format!("  Quality check failed: {e}. Discarding card."));
                // Don't keep cards when quality check fails — surface the error
            }
        }
    }
    spinner.finish_and_clear();

    if total_cost > 0.0 {
        eprintln!("  Quality check cost: {}", pricing::format_cost(total_cost));
    }

    if flagged.is_empty() {
        eprintln!("\nAll cards passed the quality check.");
        return Ok(QualityCheckResult {
            final_cards: valid_cards,
            cost: total_cost,
        });
    }

    let flagged_count = flagged.len();
    eprintln!("\n{flagged_count} card(s) were flagged. Please review:");

    for (i, (card, reason)) in flagged.into_iter().enumerate() {
        eprintln!("\n--- Flagged Card {}/{} ---", i + 1, flagged_count);
        for (key, value) in &card.fields {
            eprintln!("{key}: {value}");
        }
        eprintln!("\nReason: {reason}");

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
) -> Result<(bool, String, f64), anyhow::Error> {
    let mut last_error = String::new();

    for attempt in 0..=retries {
        if attempt > 0 {
            let backoff = Duration::from_millis(1000 * 2u64.pow(attempt - 1));
            std::thread::sleep(backoff.min(Duration::from_secs(30)));
        }

        match client.chat_completion(model, prompt, temperature, max_tokens) {
            Ok(result) => {
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

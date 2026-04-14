use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;

use crate::data::Row;
use crate::llm::client::{JsonSchema, LlmClient, ResponseFormat};
use crate::llm::error::LlmError;
use crate::llm::logger::LlmLogger;
use crate::llm::parse_json::try_parse_json_object;
use crate::llm::pricing;
use crate::template::fill_template;
use crate::template::frontmatter::{ProcessorKind, ProcessorStep};

use super::cards::ValidatedCard;
use super::processor::CardCandidate;

/// How many LLM calls to run in parallel per step.
const DEFAULT_CONCURRENCY: usize = 10;

pub struct ProcessResult {
    pub cards: Vec<CardCandidate>,
    /// Cards flagged by check steps (card_index into returned cards vec + reason).
    pub flags: Vec<CardFlag>,
    /// Number of cards rejected by check steps (already removed from cards vec).
    pub rejected_count: usize,
    /// Error messages from failed processing steps (for UI display).
    pub errors: Vec<String>,
    pub cost: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

pub struct CardFlag {
    pub card_index: usize,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CheckVerdict {
    Pass,
    Flag,
    Reject,
}

/// A card flagged during quality/check processing, kept for user review.
#[derive(Clone)]
pub struct FlaggedCard {
    pub card: ValidatedCard,
    pub reason: String,
}

/// Run a sequence of processor steps on cards. Steps execute sequentially;
/// cards within each step are processed concurrently.
#[allow(clippy::too_many_arguments)]
pub fn run_processors(
    steps: &[ProcessorStep],
    cards: Vec<CardCandidate>,
    field_map_keys: &[String],
    client: &LlmClient,
    default_model: &str,
    temperature: Option<f64>,
    max_tokens: Option<u64>,
    retries: u32,
    logger: Option<&LlmLogger>,
    on_progress: &(dyn Fn(&str) + Send + Sync),
) -> Result<ProcessResult, anyhow::Error> {
    let mut current_cards = cards;
    let mut total_cost = 0.0;
    let mut total_input_tokens = 0u64;
    let mut total_output_tokens = 0u64;
    let mut total_rejected = 0usize;
    let mut all_errors: Vec<String> = Vec::new();
    // flags track (original index in final cards vec, reason)
    let mut all_flags: Vec<CardFlag> = Vec::new();

    for (step_idx, step) in steps.iter().enumerate() {
        let model = step.model.as_deref().unwrap_or(default_model);
        let total_cards = current_cards.len();

        // Step model overrides only change the model name sent in the request.
        // The transport (base URL, API key) always comes from the main client,
        // so custom endpoints (OpenRouter, Ollama, etc.) work correctly.
        let effective_client = client;

        if total_cards == 0 {
            break;
        }

        let desc = match step.kind {
            ProcessorKind::Transform => {
                let fields = step.write_fields();
                if fields.len() == 1 {
                    format!("rewriting '{}'", fields[0])
                } else {
                    format!("updating {}", fields.join(", "))
                }
            }
            ProcessorKind::Check => "checking cards".to_string(),
        };
        on_progress(&format!(
            "Processing {}/{}: {desc} on {total_cards} card(s) using {model}...",
            step_idx + 1,
            steps.len(),
        ));

        let system_prompt = build_system_prompt(step);
        let response_format = build_response_schema(step);

        // Build user prompts for each card
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
            let filled = fill_template(&step.prompt, &row)?;
            prompts.push(filled);
        }

        // Process cards concurrently
        type StepResult = Result<(StepOutcome, u64, u64, f64), String>;
        let results: Arc<Mutex<Vec<(usize, StepResult)>>> =
            Arc::new(Mutex::new(Vec::with_capacity(total_cards)));

        let next_index = Arc::new(AtomicUsize::new(0));
        let concurrency = DEFAULT_CONCURRENCY.min(total_cards);

        std::thread::scope(|s| {
            for _ in 0..concurrency {
                let next_index = Arc::clone(&next_index);
                let results = Arc::clone(&results);
                let prompts = &prompts;
                let system_prompt = &system_prompt;
                let response_format = &response_format;

                s.spawn(move || {
                    loop {
                        let idx = next_index.fetch_add(1, Ordering::SeqCst);
                        if idx >= total_cards {
                            break;
                        }

                        let outcome = run_single_step(
                            effective_client,
                            model,
                            system_prompt,
                            &prompts[idx],
                            response_format.as_ref(),
                            step,
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

        // Apply results
        let mut next_cards = Vec::with_capacity(total_cards);
        let mut cards_iter: Vec<Option<CardCandidate>> =
            current_cards.into_iter().map(Some).collect();
        let mut step_cost = 0.0;

        for (idx, result) in results {
            let card = cards_iter[idx].take().unwrap();
            match result {
                Ok((outcome, in_tok, out_tok, cost)) => {
                    total_cost += cost;
                    step_cost += cost;
                    total_input_tokens += in_tok;
                    total_output_tokens += out_tok;

                    match outcome {
                        StepOutcome::Transform(updates) => {
                            let mut card = card;
                            let write_fields = step.write_fields();
                            for field in &write_fields {
                                let Some(new_value) = updates.get(*field) else {
                                    continue;
                                };
                                let old_value = card
                                    .fields
                                    .get(*field)
                                    .map(|v| match v {
                                        Value::String(s) => s.clone(),
                                        other => other.to_string(),
                                    })
                                    .unwrap_or_default();
                                if old_value != *new_value {
                                    emit_field_diff(
                                        on_progress,
                                        idx + 1,
                                        field,
                                        &old_value,
                                        new_value,
                                    );
                                }
                            }
                            for (key, value) in updates {
                                card.fields.insert(key, Value::String(value));
                            }
                            next_cards.push(card);
                        }
                        StepOutcome::Check(CheckVerdict::Pass, _) => {
                            next_cards.push(card);
                        }
                        StepOutcome::Check(CheckVerdict::Flag, reason) => {
                            // Card stays in pipeline but gets flagged
                            let flag_idx = next_cards.len();
                            next_cards.push(card);
                            all_flags.push(CardFlag {
                                card_index: flag_idx,
                                reason: reason.unwrap_or_default(),
                            });
                        }
                        StepOutcome::Check(CheckVerdict::Reject, reason) => {
                            total_rejected += 1;
                            on_progress(&format!(
                                "  Card {} rejected: {}",
                                idx + 1,
                                reason.as_deref().unwrap_or("no reason")
                            ));
                        }
                    }
                }
                Err(e) => {
                    total_rejected += 1;
                    let msg = format!("Processing failed for card {}: {e}", idx + 1);
                    on_progress(&format!("  {msg}. Discarding."));
                    all_errors.push(msg);
                }
            }
        }

        if step_cost > 0.0 {
            on_progress(&format!(
                "  Step {}/{} cost: {}",
                step_idx + 1,
                steps.len(),
                pricing::format_cost(step_cost)
            ));
        }

        current_cards = next_cards;
    }

    Ok(ProcessResult {
        cards: current_cards,
        flags: all_flags,
        rejected_count: total_rejected,
        errors: all_errors,
        cost: total_cost,
        input_tokens: total_input_tokens,
        output_tokens: total_output_tokens,
    })
}

#[derive(Debug)]
enum StepOutcome {
    Transform(HashMap<String, String>),
    Check(CheckVerdict, Option<String>),
}

/// Emit a per-line before/after diff for a transformed field via the
/// progress callback. One log line per emitted row so the TUI log panel
/// renders each on its own line.
fn emit_field_diff(
    on_progress: &(dyn Fn(&str) + Send + Sync),
    card_num: usize,
    field: &str,
    old: &str,
    new: &str,
) {
    on_progress(&format!("  Card {card_num} [{field}]:"));
    if old.is_empty() {
        on_progress("    - (empty)");
    } else {
        for line in old.lines() {
            on_progress(&format!("    - {line}"));
        }
    }
    if new.is_empty() {
        on_progress("    + (empty)");
    } else {
        for line in new.lines() {
            on_progress(&format!("    + {line}"));
        }
    }
}

fn build_system_prompt(step: &ProcessorStep) -> String {
    match step.kind {
        ProcessorKind::Check => {
            "You are a card quality evaluator. Evaluate the card and respond with JSON:\n\
             {\"result\": \"pass\" | \"flag\" | \"reject\", \"reason\": \"brief explanation\"}\n\n\
             - \"pass\": card is good\n\
             - \"flag\": card has minor issues worth noting but is usable\n\
             - \"reject\": card has serious problems and should be discarded"
                .to_string()
        }
        ProcessorKind::Transform => {
            let writes = step.write_fields();
            if writes.len() == 1 {
                format!(
                    "You are a card field processor. Respond with JSON:\n\
                     {{\"{}\": \"your result\"}}\n\n\
                     Return ONLY the JSON object, no other text.",
                    writes[0]
                )
            } else {
                let keys: Vec<_> = writes.iter().map(|w| format!("\"{w}\"")).collect();
                format!(
                    "You are a card field processor. Respond with a JSON object containing these keys: [{}]\n\n\
                     Return ONLY the JSON object, no other text.",
                    keys.join(", ")
                )
            }
        }
    }
}

fn build_response_schema(step: &ProcessorStep) -> Option<ResponseFormat> {
    let schema = match step.kind {
        ProcessorKind::Check => serde_json::json!({
            "type": "object",
            "properties": {
                "result": {
                    "type": "string",
                    "enum": ["pass", "flag", "reject"]
                },
                "reason": {
                    "type": "string"
                }
            },
            "required": ["result", "reason"],
            "additionalProperties": false
        }),
        ProcessorKind::Transform => {
            let writes = step.write_fields();
            let mut properties = serde_json::Map::new();
            let mut required = Vec::new();
            for field in &writes {
                properties.insert(field.to_string(), serde_json::json!({"type": "string"}));
                required.push(serde_json::Value::String(field.to_string()));
            }
            serde_json::json!({
                "type": "object",
                "properties": properties,
                "required": required,
                "additionalProperties": false
            })
        }
    };

    let name = match step.kind {
        ProcessorKind::Check => "check_result",
        ProcessorKind::Transform => "transform_result",
    };

    Some(ResponseFormat::JsonSchema {
        json_schema: JsonSchema {
            name: name.to_string(),
            schema,
            strict: true,
        },
    })
}

/// Run a single LLM call for one card in one step.
#[allow(clippy::too_many_arguments)]
fn run_single_step(
    client: &LlmClient,
    model: &str,
    system_prompt: &str,
    user_prompt: &str,
    response_format: Option<&ResponseFormat>,
    step: &ProcessorStep,
    field_map_keys: &[String],
    temperature: Option<f64>,
    max_tokens: Option<u64>,
    retries: u32,
    logger: Option<&LlmLogger>,
) -> Result<(StepOutcome, u64, u64, f64), anyhow::Error> {
    let mut last_error = String::new();

    for attempt in 0..=retries {
        if attempt > 0 {
            let backoff = Duration::from_millis(1000 * 2u64.pow(attempt - 1));
            std::thread::sleep(backoff.min(Duration::from_secs(30)));
        }

        match client.chat_completion_structured(
            model,
            Some(system_prompt),
            user_prompt,
            temperature,
            max_tokens,
            response_format,
        ) {
            Ok(result) => {
                if let Some(logger) = logger {
                    logger.log(user_prompt, &result.content);
                }

                let outcome = parse_step_result(&result.content, step, field_map_keys)?;

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

                return Ok((outcome, in_tok, out_tok, cost));
            }
            Err(e) => {
                last_error = e.to_string();
                if let Some(logger) = logger {
                    logger.log_error(user_prompt, &last_error);
                }
                if let LlmError::Api(_) = e {
                    break;
                }
            }
        }
    }

    anyhow::bail!("Processing failed: {last_error}")
}

fn parse_step_result(
    content: &str,
    step: &ProcessorStep,
    field_map_keys: &[String],
) -> Result<StepOutcome, anyhow::Error> {
    let trimmed = content.trim();

    match step.kind {
        ProcessorKind::Check => {
            let obj = try_parse_json_object(trimmed)
                .ok_or_else(|| anyhow::anyhow!("Check response is not valid JSON"))?;

            let result_str = obj
                .get("result")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("Missing 'result' in check response"))?;

            let reason = obj
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("No reason provided")
                .to_string();

            let verdict = match result_str {
                "pass" => CheckVerdict::Pass,
                "flag" => CheckVerdict::Flag,
                "reject" => CheckVerdict::Reject,
                other => anyhow::bail!("Unknown check result: '{other}'"),
            };

            Ok(StepOutcome::Check(verdict, Some(reason)))
        }
        ProcessorKind::Transform => {
            let write_fields = step.write_fields();
            let obj = try_parse_json_object(trimmed)
                .ok_or_else(|| anyhow::anyhow!("Transform response is not valid JSON"))?;

            let mut updates = HashMap::new();
            for field in &write_fields {
                let value = obj
                    .get(*field)
                    .ok_or_else(|| anyhow::anyhow!("Transform response missing field '{field}'"))?;
                let text = match value {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                updates.insert(field.to_string(), text);
            }

            // Check for unexpected keys
            for key in obj.keys() {
                if !write_fields.contains(&key.as_str()) && !field_map_keys.contains(key) {
                    anyhow::bail!("Transform returned unknown field '{key}'");
                }
            }

            Ok(StepOutcome::Transform(updates))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::template::frontmatter::ProcessorKind;

    fn keys() -> Vec<String> {
        vec![
            "front".into(),
            "kanji".into(),
            "read".into(),
            "context".into(),
        ]
    }

    fn make_step(kind: ProcessorKind, target: Option<&str>, writes: Vec<&str>) -> ProcessorStep {
        ProcessorStep {
            kind,
            target: target.map(|s| s.to_string()),
            writes: writes.into_iter().map(|s| s.to_string()).collect(),
            prompt: String::new(),
            model: None,
        }
    }

    #[test]
    fn parse_check_pass() {
        let step = make_step(ProcessorKind::Check, None, vec![]);
        let content = r#"{"result": "pass", "reason": "looks good"}"#;
        match parse_step_result(content, &step, &keys()).unwrap() {
            StepOutcome::Check(CheckVerdict::Pass, reason) => {
                assert_eq!(reason.unwrap(), "looks good");
            }
            _ => panic!("expected Check Pass"),
        }
    }

    #[test]
    fn parse_check_flag() {
        let step = make_step(ProcessorKind::Check, None, vec![]);
        let content = r#"{"result": "flag", "reason": "minor issue"}"#;
        match parse_step_result(content, &step, &keys()).unwrap() {
            StepOutcome::Check(CheckVerdict::Flag, reason) => {
                assert_eq!(reason.unwrap(), "minor issue");
            }
            _ => panic!("expected Check Flag"),
        }
    }

    #[test]
    fn parse_check_reject() {
        let step = make_step(ProcessorKind::Check, None, vec![]);
        let content = r#"{"result": "reject", "reason": "completely wrong"}"#;
        match parse_step_result(content, &step, &keys()).unwrap() {
            StepOutcome::Check(CheckVerdict::Reject, reason) => {
                assert_eq!(reason.unwrap(), "completely wrong");
            }
            _ => panic!("expected Check Reject"),
        }
    }

    #[test]
    fn parse_transform_single_target() {
        let step = make_step(ProcessorKind::Transform, Some("read"), vec![]);
        let content = r#"{"read": "何[なに]か あったら"}"#;
        match parse_step_result(content, &step, &keys()).unwrap() {
            StepOutcome::Transform(updates) => {
                assert_eq!(updates.len(), 1);
                assert_eq!(updates["read"], "何[なに]か あったら");
            }
            _ => panic!("expected Transform"),
        }
    }

    #[test]
    fn parse_transform_multi_writes() {
        let step = make_step(ProcessorKind::Transform, None, vec!["read", "context"]);
        let content = r#"{"read": "何[なに]か", "context": "Casual"}"#;
        match parse_step_result(content, &step, &keys()).unwrap() {
            StepOutcome::Transform(updates) => {
                assert_eq!(updates.len(), 2);
                assert_eq!(updates["read"], "何[なに]か");
                assert_eq!(updates["context"], "Casual");
            }
            _ => panic!("expected Transform"),
        }
    }

    #[test]
    fn parse_transform_missing_field_errors() {
        let step = make_step(ProcessorKind::Transform, None, vec!["read", "context"]);
        let content = r#"{"read": "test"}"#;
        assert!(parse_step_result(content, &step, &keys()).is_err());
    }

    #[test]
    fn parse_check_invalid_json_errors() {
        let step = make_step(ProcessorKind::Check, None, vec![]);
        let content = "not json";
        assert!(parse_step_result(content, &step, &keys()).is_err());
    }

    #[test]
    fn build_check_schema() {
        let step = make_step(ProcessorKind::Check, None, vec![]);
        let fmt = build_response_schema(&step).unwrap();
        match fmt {
            ResponseFormat::JsonSchema { json_schema } => {
                assert_eq!(json_schema.name, "check_result");
                assert!(json_schema.strict);
            }
        }
    }

    #[test]
    fn emit_diff_single_line() {
        let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let captured_clone = Arc::clone(&captured);
        let on_log: Box<dyn Fn(&str) + Send + Sync> = Box::new(move |msg: &str| {
            captured_clone.lock().unwrap().push(msg.to_string());
        });
        emit_field_diff(
            on_log.as_ref(),
            1,
            "read",
            "日本語を勉強しています",
            "日本語[にほんご]を 勉強[べんきょう]しています",
        );
        let logs = captured.lock().unwrap();
        assert_eq!(logs.len(), 3);
        assert_eq!(logs[0], "  Card 1 [read]:");
        assert_eq!(logs[1], "    - 日本語を勉強しています");
        assert_eq!(
            logs[2],
            "    + 日本語[にほんご]を 勉強[べんきょう]しています"
        );
    }

    #[test]
    fn emit_diff_multiline() {
        let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let captured_clone = Arc::clone(&captured);
        let on_log: Box<dyn Fn(&str) + Send + Sync> = Box::new(move |msg: &str| {
            captured_clone.lock().unwrap().push(msg.to_string());
        });
        emit_field_diff(
            on_log.as_ref(),
            2,
            "context",
            "line one\nline two",
            "line one\nchanged",
        );
        let logs = captured.lock().unwrap();
        assert_eq!(logs.len(), 5);
        assert_eq!(logs[0], "  Card 2 [context]:");
        assert_eq!(logs[1], "    - line one");
        assert_eq!(logs[2], "    - line two");
        assert_eq!(logs[3], "    + line one");
        assert_eq!(logs[4], "    + changed");
    }

    #[test]
    fn emit_diff_empty_old() {
        let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let captured_clone = Arc::clone(&captured);
        let on_log: Box<dyn Fn(&str) + Send + Sync> = Box::new(move |msg: &str| {
            captured_clone.lock().unwrap().push(msg.to_string());
        });
        emit_field_diff(on_log.as_ref(), 3, "read", "", "something");
        let logs = captured.lock().unwrap();
        assert_eq!(logs.len(), 3);
        assert_eq!(logs[1], "    - (empty)");
        assert_eq!(logs[2], "    + something");
    }

    #[test]
    fn build_transform_schema() {
        let step = make_step(ProcessorKind::Transform, Some("read"), vec![]);
        let fmt = build_response_schema(&step).unwrap();
        match fmt {
            ResponseFormat::JsonSchema { json_schema } => {
                assert_eq!(json_schema.name, "transform_result");
                let props = json_schema.schema["properties"].as_object().unwrap();
                assert!(props.contains_key("read"));
            }
        }
    }
}

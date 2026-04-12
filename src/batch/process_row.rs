use std::sync::Arc;

use serde_json::Value;

use crate::data::rows::Row;
use crate::llm::client::LlmClient;
use crate::llm::error::LlmError;
use crate::llm::extract::extract_result_tag;
use crate::llm::logger::LlmLogger;
use crate::llm::parse_json::{merge_fields_case_insensitive, try_parse_json_object};
use crate::template::fill_template;

use super::engine::ProcessFn;
use super::error::BatchError;
use super::report::ERROR_FIELD;

/// Configuration for building a row-processing closure.
pub struct ProcessRowConfig {
    pub client: Arc<LlmClient>,
    pub model: String,
    pub template: String,
    pub field: Option<String>,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u64>,
    pub require_result_tag: bool,
    pub logger: Option<Arc<LlmLogger>>,
}

/// Build the closure that processes a single row through the LLM.
/// Used by both process-file and process-deck commands.
pub fn build_process_fn(config: ProcessRowConfig) -> ProcessFn {
    Arc::new(move |row: &Row| {
        // Fill template
        let prompt =
            fill_template(&config.template, row).map_err(|e| BatchError::Fatal(e.to_string()))?;

        // Call LLM — Api errors (4xx) are fatal, Http/Decode are retryable
        let result = config
            .client
            .chat_completion(
                &config.model,
                &prompt,
                config.temperature,
                config.max_tokens,
            )
            .map_err(|e| match e {
                LlmError::Api(_) => BatchError::Fatal(e.to_string()),
                _ => BatchError::Processing(e.to_string()),
            })?;

        let response_text = result.content;

        if let Some(ref logger) = config.logger {
            logger.log(&prompt, &response_text);
        }

        let usage = result.usage.map(|u| (u.prompt_tokens, u.completion_tokens));

        // Extract from result tags if configured
        let processed_text = extract_result_tag(&response_text, config.require_result_tag)
            .map_err(BatchError::Processing)?;

        // Build output row
        let mut output_row = row.clone();

        if let Some(ref field_name) = config.field {
            // Single field mode
            output_row.insert(field_name.clone(), Value::String(processed_text));
        } else {
            // JSON merge mode
            let parsed = try_parse_json_object(&processed_text).ok_or_else(|| {
                let preview = if processed_text.is_empty() {
                    "(empty response)".to_string()
                } else {
                    processed_text.chars().take(200).collect::<String>()
                };
                BatchError::Processing(format!("LLM response is not valid JSON: {preview}"))
            })?;
            merge_fields_case_insensitive(&mut output_row, &parsed).map_err(BatchError::Fatal)?;
        }

        // Remove error field if present from a previous failed attempt
        output_row.shift_remove(ERROR_FIELD);

        Ok((output_row, usage))
    })
}

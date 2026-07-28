use std::sync::Arc;

use serde_json::Value;

use crate::data::rows::Row;
use crate::llm::client::{JsonSchema, LlmClient, ResponseFormat};
use crate::llm::error::LlmError;
use crate::llm::extract::extract_result_tag;
use crate::llm::logger::LlmLogger;
use crate::llm::parse_json::try_parse_json_object;
use crate::template::fill_template;

use super::engine::ProcessFn;
use super::error::BatchError;
use super::report::ERROR_FIELD;

/// Configuration for building a row-processing closure.
pub struct ProcessRowConfig {
    pub client: Arc<LlmClient>,
    pub model: String,
    pub template: String,
    /// Field names to receive the LLM response.
    pub fields: Vec<String>,
    /// Whether the response uses a JSON object containing all configured fields.
    pub structured: bool,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u64>,
    pub require_result_tag: bool,
    pub logger: Option<Arc<LlmLogger>>,
}

/// Build the closure that processes a single row through the LLM.
/// Used by both process-file and process-deck commands.
pub fn build_process_fn(config: ProcessRowConfig) -> ProcessFn {
    let response_format = config
        .structured
        .then(|| build_response_format(&config.fields));
    let system_prompt = config
        .structured
        .then(|| build_system_prompt(&config.fields));

    Arc::new(move |row: &Row| {
        let prompt =
            fill_template(&config.template, row).map_err(|e| BatchError::Fatal(e.to_string()))?;

        let result = config
            .client
            .chat_completion_structured(
                &config.model,
                system_prompt.as_deref(),
                &prompt,
                config.temperature,
                config.max_tokens,
                response_format.as_ref(),
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
        let updates = if config.structured {
            parse_structured_response(&response_text, &config.fields)
                .map_err(BatchError::Processing)?
        } else {
            let processed_text = extract_result_tag(&response_text, config.require_result_tag)
                .map_err(BatchError::Processing)?;
            vec![(config.fields[0].clone(), processed_text)]
        };

        // Apply validated updates together so a malformed response cannot
        // partially modify a row.
        let mut output_row = row.clone();
        for (field, value) in updates {
            output_row.insert(field, Value::String(value));
        }
        output_row.shift_remove(ERROR_FIELD);

        Ok((output_row, usage))
    })
}

fn build_system_prompt(fields: &[String]) -> String {
    format!(
        "Return only a JSON object with exactly these string fields: {}.",
        fields.join(", ")
    )
}

fn build_response_format(fields: &[String]) -> ResponseFormat {
    let properties: serde_json::Map<String, Value> = fields
        .iter()
        .map(|field| (field.clone(), serde_json::json!({"type": "string"})))
        .collect();
    let required: Vec<Value> = fields.iter().cloned().map(Value::String).collect();

    ResponseFormat::JsonSchema {
        json_schema: JsonSchema {
            name: "process_fields".into(),
            schema: serde_json::json!({
                "type": "object",
                "properties": properties,
                "required": required,
                "additionalProperties": false
            }),
            strict: true,
        },
    }
}

fn parse_structured_response(
    response: &str,
    fields: &[String],
) -> Result<Vec<(String, String)>, String> {
    let object = try_parse_json_object(response.trim())
        .ok_or_else(|| "response is not a valid JSON object".to_string())?;

    for key in object.keys() {
        if !fields.iter().any(|field| field == key) {
            return Err(format!("response contains undeclared field '{key}'"));
        }
    }

    fields
        .iter()
        .map(|field| {
            let value = object
                .get(field)
                .ok_or_else(|| format!("response is missing field '{field}'"))?
                .as_str()
                .ok_or_else(|| format!("response field '{field}' must be a string"))?;
            if value.trim().is_empty() {
                return Err(format!("response field '{field}' must be non-empty"));
            }
            Ok((field.clone(), value.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::runtime::RuntimeConfig;
    use serde_json::json;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    fn client_with_response(content: &str) -> (Arc<LlmClient>, mpsc::Receiver<Value>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let content = content.to_string();
        let (request_tx, request_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 4096];
            let (body_start, content_length) = loop {
                let count = stream.read(&mut buffer).unwrap();
                request.extend_from_slice(&buffer[..count]);
                let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n")
                else {
                    continue;
                };
                let body_start = header_end + 4;
                let headers = String::from_utf8_lossy(&request[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .and_then(|value| value.parse::<usize>().ok())
                    })
                    .unwrap();
                if request.len() >= body_start + content_length {
                    break (body_start, content_length);
                }
            };
            let body: Value =
                serde_json::from_slice(&request[body_start..body_start + content_length]).unwrap();
            request_tx.send(body).unwrap();

            let response_body = serde_json::to_string(&json!({
                "choices": [{"message": {"content": content}}],
                "usage": {"prompt_tokens": 11, "completion_tokens": 7}
            }))
            .unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            )
            .unwrap();
        });

        let runtime = RuntimeConfig {
            model: "test-model".into(),
            api_key: None,
            api_base_url: Some(format!("http://{address}")),
            temperature: None,
            max_tokens: None,
            batch_size: 1,
            retries: 0,
            gemini_thinking_enabled: false,
        };
        (Arc::new(LlmClient::from_config(&runtime)), request_rx)
    }

    fn process_config(
        client: Arc<LlmClient>,
        fields: &[&str],
        structured: bool,
    ) -> ProcessRowConfig {
        ProcessRowConfig {
            client,
            model: "test-model".into(),
            template: "Sentence: {Kanji}\nMetadata: {SourceOnly}".into(),
            fields: fields.iter().map(|field| field.to_string()).collect(),
            structured,
            temperature: None,
            max_tokens: None,
            require_result_tag: false,
            logger: None,
        }
    }

    #[test]
    fn multi_field_processing_is_atomic_and_preserves_undeclared_fields() {
        let (client, request_rx) =
            client_with_response(r#"{"Reading":"漢[かん]字[じ]","Explanation":"uses kanji"}"#);
        let process = build_process_fn(process_config(client, &["Reading", "Explanation"], true));
        let row = Row::from([
            ("Kanji".into(), json!("漢字")),
            ("Reading".into(), json!("")),
            ("Explanation".into(), json!("")),
            ("SourceOnly".into(), json!("authoritative metadata")),
        ]);

        let (output, usage) = process(&row).unwrap();

        assert_eq!(output["Reading"], "漢[かん]字[じ]");
        assert_eq!(output["Explanation"], "uses kanji");
        assert_eq!(output["Kanji"], row["Kanji"]);
        assert_eq!(output["SourceOnly"], row["SourceOnly"]);
        assert_eq!(usage, Some((11, 7)));
        let request = request_rx.recv().unwrap();
        assert!(
            request["messages"][1]["content"]
                .as_str()
                .unwrap()
                .contains("authoritative metadata")
        );
        assert_eq!(
            request["response_format"]["json_schema"]["schema"]["required"],
            json!(["Reading", "Explanation"])
        );
    }

    #[test]
    fn missing_multi_field_fails_without_mutating_input() {
        let (client, _) = client_with_response(r#"{"Reading":"read"}"#);
        let process = build_process_fn(process_config(client, &["Reading", "Explanation"], true));
        let row = Row::from([
            ("Kanji".into(), json!("漢字")),
            ("Reading".into(), json!("original reading")),
            ("Explanation".into(), json!("original explanation")),
            ("SourceOnly".into(), json!("metadata")),
        ]);
        let original = row.clone();

        assert!(process(&row).is_err());
        assert_eq!(row, original);
    }

    #[test]
    fn single_field_prompt_accepts_plain_text_response() {
        let (client, request_rx) = client_with_response("generated hint");
        let process = build_process_fn(process_config(client, &["Hint"], false));
        let row = Row::from([
            ("Kanji".into(), json!("漢字")),
            ("SourceOnly".into(), json!("metadata")),
        ]);

        let (output, _) = process(&row).unwrap();

        assert_eq!(output["Hint"], "generated hint");
        assert_eq!(output["Kanji"], row["Kanji"]);
        let request = request_rx.recv().unwrap();
        assert!(request.get("response_format").is_none());
        assert_eq!(request["messages"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn parses_all_declared_fields_in_order() {
        let fields = vec!["Reading".into(), "Explanation".into()];
        let updates = parse_structured_response(
            r#"{"Explanation":"because","Reading":"漢[かん]字[じ]"}"#,
            &fields,
        )
        .unwrap();

        assert_eq!(
            updates,
            vec![
                ("Reading".into(), "漢[かん]字[じ]".into()),
                ("Explanation".into(), "because".into())
            ]
        );
    }

    #[test]
    fn rejects_missing_field_without_updates() {
        let fields = vec!["Reading".into(), "Explanation".into()];
        let error = parse_structured_response(r#"{"Reading":"read"}"#, &fields).unwrap_err();
        assert!(error.contains("Explanation"));
    }

    #[test]
    fn rejects_non_string_and_empty_fields() {
        let fields = vec!["Reading".into()];
        assert!(parse_structured_response(r#"{"Reading":42}"#, &fields).is_err());
        assert!(parse_structured_response(r#"{"Reading":"  "}"#, &fields).is_err());
    }

    #[test]
    fn rejects_undeclared_response_fields() {
        let fields = vec!["Reading".into()];
        let error =
            parse_structured_response(r#"{"Reading":"read","Front":"replacement"}"#, &fields)
                .unwrap_err();
        assert!(error.contains("Front"));
    }

    #[test]
    fn response_schema_requires_every_field() {
        let fields = vec!["Reading".into(), "Explanation".into()];
        let ResponseFormat::JsonSchema { json_schema } = build_response_format(&fields);
        assert_eq!(json_schema.schema["required"], json!(fields));
        assert_eq!(json_schema.schema["additionalProperties"], false);
    }
}

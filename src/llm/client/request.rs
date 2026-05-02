use serde::Serialize;

use crate::llm::provider::ThinkingFormat;

#[derive(Debug, Clone, Serialize)]
pub struct JsonSchema {
    pub name: String,
    pub schema: serde_json::Value,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub strict: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseFormat {
    JsonSchema { json_schema: JsonSchema },
}

pub(super) fn structured_chat_request<'a>(
    model: &'a str,
    system_prompt: Option<&'a str>,
    user_prompt: &'a str,
    temperature: Option<f64>,
    max_tokens: Option<u64>,
    response_format: Option<&'a ResponseFormat>,
) -> ChatRequest<'a> {
    let mut messages = Vec::new();
    if let Some(sys) = system_prompt {
        messages.push(ChatMessage {
            role: "system",
            content: sys,
        });
    }
    messages.push(ChatMessage {
        role: "user",
        content: user_prompt,
    });

    ChatRequest {
        model,
        messages,
        temperature,
        max_tokens,
        response_format,
        stream: None,
        stream_options: None,
        extra_body: None,
    }
}

pub(super) fn thinking_stream_chat_request<'a>(
    model: &'a str,
    prompt: &'a str,
    temperature: Option<f64>,
    max_tokens: Option<u64>,
    format: ThinkingFormat,
) -> ChatRequest<'a> {
    ChatRequest {
        model,
        messages: vec![ChatMessage {
            role: "user",
            content: prompt,
        }],
        temperature,
        max_tokens,
        response_format: None,
        stream: Some(true),
        stream_options: Some(StreamOptions {
            include_usage: true,
        }),
        extra_body: match format {
            ThinkingFormat::GeminiThoughtTags => Some(serde_json::json!({
                "google": {
                    "thinking_config": {
                        "thinking_level": "high",
                        "include_thoughts": true
                    }
                }
            })),
            ThinkingFormat::ReasoningContent => None,
        },
    }
}

#[derive(Debug, Serialize)]
pub(super) struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<&'a ResponseFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<StreamOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    extra_body: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct StreamOptions {
    include_usage: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_request_serializes_system_user_and_schema() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "answer": { "type": "string" }
            },
            "required": ["answer"]
        });
        let json_schema = JsonSchema {
            name: "answer_schema".to_string(),
            schema: schema.clone(),
            strict: true,
        };
        let response_format = ResponseFormat::JsonSchema { json_schema };
        let request = structured_chat_request(
            "gpt-5",
            Some("system prompt"),
            "user prompt",
            Some(0.2),
            Some(512),
            Some(&response_format),
        );

        assert_eq!(
            serde_json::to_value(request).unwrap(),
            serde_json::json!({
                "model": "gpt-5",
                "messages": [
                    { "role": "system", "content": "system prompt" },
                    { "role": "user", "content": "user prompt" }
                ],
                "temperature": 0.2,
                "max_tokens": 512,
                "response_format": {
                    "type": "json_schema",
                    "json_schema": {
                        "name": "answer_schema",
                        "schema": schema,
                        "strict": true
                    }
                }
            })
        );
    }

    #[test]
    fn structured_request_omits_optional_fields_when_absent() {
        let request = structured_chat_request("gpt-5", None, "user prompt", None, None, None);

        assert_eq!(
            serde_json::to_value(request).unwrap(),
            serde_json::json!({
                "model": "gpt-5",
                "messages": [
                    { "role": "user", "content": "user prompt" }
                ]
            })
        );
    }

    #[test]
    fn gemini_thinking_stream_request_serializes_extra_body() {
        let request = thinking_stream_chat_request(
            "gemini-2.5-flash",
            "think aloud",
            Some(0.4),
            Some(256),
            ThinkingFormat::GeminiThoughtTags,
        );

        assert_eq!(
            serde_json::to_value(request).unwrap(),
            serde_json::json!({
                "model": "gemini-2.5-flash",
                "messages": [
                    { "role": "user", "content": "think aloud" }
                ],
                "temperature": 0.4,
                "max_tokens": 256,
                "stream": true,
                "stream_options": { "include_usage": true },
                "extra_body": {
                    "google": {
                        "thinking_config": {
                            "thinking_level": "high",
                            "include_thoughts": true
                        }
                    }
                }
            })
        );
    }

    #[test]
    fn reasoning_content_stream_request_has_no_extra_body() {
        let request = thinking_stream_chat_request(
            "deepseek-v4-pro",
            "think aloud",
            None,
            None,
            ThinkingFormat::ReasoningContent,
        );

        assert_eq!(
            serde_json::to_value(request).unwrap(),
            serde_json::json!({
                "model": "deepseek-v4-pro",
                "messages": [
                    { "role": "user", "content": "think aloud" }
                ],
                "stream": true,
                "stream_options": { "include_usage": true }
            })
        );
    }
}

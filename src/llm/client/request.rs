use serde::Serialize;

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

#[derive(Debug, Serialize)]
pub(super) struct ChatRequest<'a> {
    pub model: &'a str,
    pub messages: Vec<ChatMessage<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<&'a ResponseFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<StreamOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra_body: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub(super) struct ChatMessage<'a> {
    pub role: &'a str,
    pub content: &'a str,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub(super) struct StreamOptions {
    pub include_usage: bool,
}

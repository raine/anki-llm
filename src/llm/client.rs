use std::io::Read;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::error::LlmError;
use crate::llm::provider::{self, ThinkingFormat};
use crate::llm::runtime::RuntimeConfig;
use crate::llm::sse::SseParser;

const DEFAULT_OPENAI_BASE: &str = "https://api.openai.com/v1";
const TIMEOUT_SECS: u64 = 90;
const STREAM_READ_TIMEOUT_SECS: u64 = 30;

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
struct ChatRequest<'a> {
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

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
    usage: Option<ChatUsage>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatChoiceMessage,
}

#[derive(Debug, Deserialize)]
struct ChatChoiceMessage {
    content: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    #[serde(default)]
    total_tokens: Option<u64>,
    #[serde(default)]
    completion_tokens_details: Option<CompletionTokensDetails>,
}

#[derive(Debug, Clone, Deserialize)]
struct CompletionTokensDetails {
    #[serde(default)]
    reasoning_tokens: Option<u64>,
}

/// Result of a chat completion call.
pub struct ChatCompletionResult {
    pub content: String,
    pub usage: Option<ChatUsage>,
}

#[derive(Deserialize)]
struct ChatChunk {
    #[serde(default)]
    choices: Vec<ChatChunkChoice>,
    usage: Option<ChatUsage>,
}

#[derive(Deserialize)]
struct ChatChunkChoice {
    delta: ChatDelta,
}

#[derive(Deserialize, Default)]
struct ChatDelta {
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
}

#[derive(Debug, PartialEq)]
enum Segment {
    Thinking(String),
    Answer(String),
}

struct TagSplitter {
    open_tag: &'static str,
    close_tag: &'static str,
    in_thinking: bool,
    buffer: String,
}

impl TagSplitter {
    fn new(open_tag: &'static str, close_tag: &'static str) -> Self {
        Self {
            open_tag,
            close_tag,
            in_thinking: false,
            buffer: String::new(),
        }
    }

    fn push(&mut self, chunk: &str) -> Vec<Segment> {
        self.buffer.push_str(chunk);
        let mut out = Vec::new();
        loop {
            let target = if self.in_thinking {
                self.close_tag
            } else {
                self.open_tag
            };
            if let Some(idx) = self.buffer.find(target) {
                if !self.in_thinking && idx > 0 && !self.buffer[..idx].trim().is_empty() {
                    let emit_len = idx + target.len();
                    let segment: String = self.buffer.drain(..emit_len).collect();
                    out.push(Segment::Answer(segment));
                    continue;
                }
                if idx > 0 {
                    let segment: String = self.buffer.drain(..idx).collect();
                    out.push(self.classify(segment));
                }
                self.buffer.drain(..target.len());
                self.in_thinking = !self.in_thinking;
                if !self.in_thinking && self.buffer.starts_with('\n') {
                    self.buffer.drain(..1);
                }
            } else {
                let hold = partial_suffix_len(&self.buffer, target);
                let emit_len = self.buffer.len() - hold;
                if emit_len > 0 {
                    let segment: String = self.buffer.drain(..emit_len).collect();
                    out.push(self.classify(segment));
                }
                break;
            }
        }
        out
    }

    fn flush(mut self) -> Option<Segment> {
        if self.buffer.is_empty() {
            None
        } else {
            let text = std::mem::take(&mut self.buffer);
            Some(self.classify(text))
        }
    }

    fn classify(&self, text: String) -> Segment {
        if self.in_thinking {
            Segment::Thinking(text)
        } else {
            Segment::Answer(text)
        }
    }
}

fn partial_suffix_len(buf: &str, tag: &str) -> usize {
    let max = std::cmp::min(tag.len() - 1, buf.len());
    for i in (1..=max).rev() {
        if buf.ends_with(&tag[..i]) {
            return i;
        }
    }
    0
}

struct StreamReducer {
    splitter: Option<TagSplitter>,
    content: String,
    usage: Option<ChatUsage>,
}

impl StreamReducer {
    fn new(format: ThinkingFormat) -> Self {
        let splitter = match format {
            ThinkingFormat::GeminiThoughtTags => Some(TagSplitter::new("<thought>", "</thought>")),
            ThinkingFormat::ReasoningContent => None,
        };
        Self {
            splitter,
            content: String::new(),
            usage: None,
        }
    }

    fn apply_chunk(&mut self, chunk: ChatChunk, on_thinking: &mut impl FnMut(&str)) {
        if let Some(usage) = chunk.usage {
            self.usage = Some(effective_usage(usage));
        }
        let Some(choice) = chunk.choices.first() else {
            return;
        };
        let delta = &choice.delta;
        if let Some(thinking) = delta.reasoning_content.as_deref()
            && !thinking.is_empty()
        {
            on_thinking(thinking);
        }
        if let Some(text) = delta.content.as_deref()
            && !text.is_empty()
        {
            match self.splitter.as_mut() {
                Some(splitter) => {
                    for segment in splitter.push(text) {
                        self.apply_segment(segment, on_thinking);
                    }
                }
                None => self.content.push_str(text),
            }
        }
    }

    fn finish(mut self, on_thinking: &mut impl FnMut(&str)) -> ChatCompletionResult {
        if let Some(splitter) = self.splitter.take()
            && let Some(segment) = splitter.flush()
        {
            self.apply_segment(segment, on_thinking);
        }
        ChatCompletionResult {
            content: self.content.trim().to_string(),
            usage: self.usage,
        }
    }

    fn apply_segment(&mut self, segment: Segment, on_thinking: &mut impl FnMut(&str)) {
        match segment {
            Segment::Thinking(text) => {
                if !text.is_empty() {
                    on_thinking(&text);
                }
            }
            Segment::Answer(text) => self.content.push_str(&text),
        }
    }
}

fn effective_usage(mut usage: ChatUsage) -> ChatUsage {
    if let Some(details) = &usage.completion_tokens_details
        && let Some(reasoning_tokens) = details.reasoning_tokens
    {
        usage.completion_tokens = usage.completion_tokens.max(reasoning_tokens);
    }
    if let Some(total) = usage.total_tokens {
        let effective_completion = total.saturating_sub(usage.prompt_tokens);
        usage.completion_tokens = usage.completion_tokens.max(effective_completion);
    }
    usage
}

#[cfg(test)]
fn read_stream_completion<R: Read>(
    mut reader: R,
    format: ThinkingFormat,
    idle_timeout_secs: u64,
    on_thinking: &mut impl FnMut(&str),
    now: impl Fn() -> Instant,
) -> Result<ChatCompletionResult, LlmError> {
    let mut buf = [0u8; 8192];
    let mut parser = SseParser::new();
    let mut reducer = StreamReducer::new(format);
    let mut last_activity = now();
    let idle_timeout = Duration::from_secs(idle_timeout_secs);

    let mut done = false;
    loop {
        let n = reader.read(&mut buf).map_err(|e| {
            if is_timeout_err(&e) {
                LlmError::Http(format!("stream idle timeout after {idle_timeout_secs}s"))
            } else {
                LlmError::Http(e.to_string())
            }
        })?;
        if n == 0 {
            break;
        }
        last_activity = process_stream_bytes(
            &mut parser,
            &mut reducer,
            &buf[..n],
            on_thinking,
            &now,
            &mut done,
        )?
        .unwrap_or(last_activity);
        if done {
            break;
        }
        if now().duration_since(last_activity) >= idle_timeout {
            return Err(LlmError::Http(format!(
                "stream idle timeout after {idle_timeout_secs}s"
            )));
        }
    }

    if !done {
        return Err(LlmError::Http("stream ended before [DONE]".into()));
    }

    if let Some(event) = parser.flush()
        && event.data != "[DONE]"
    {
        let chunk = serde_json::from_str::<ChatChunk>(&event.data)
            .map_err(|e| LlmError::Decode(e.to_string()))?;
        reducer.apply_chunk(chunk, on_thinking);
    }
    Ok(reducer.finish(on_thinking))
}

fn process_stream_bytes(
    parser: &mut SseParser,
    reducer: &mut StreamReducer,
    bytes: &[u8],
    on_thinking: &mut impl FnMut(&str),
    now: &impl Fn() -> Instant,
    done: &mut bool,
) -> Result<Option<Instant>, LlmError> {
    let events = parser
        .feed(bytes)
        .map_err(|e| LlmError::Decode(e.to_string()))?;
    let mut last_activity = None;
    for event in events {
        last_activity = Some(now());
        if event.data == "[DONE]" {
            *done = true;
            break;
        }
        let chunk = serde_json::from_str::<ChatChunk>(&event.data)
            .map_err(|e| LlmError::Decode(e.to_string()))?;
        reducer.apply_chunk(chunk, on_thinking);
    }
    Ok(last_activity)
}

struct StreamReadResult {
    bytes: Vec<u8>,
    done: bool,
}

fn read_stream_completion_with_idle_timeout<R: Read + Send + 'static>(
    mut reader: R,
    format: ThinkingFormat,
    idle_timeout_secs: u64,
    on_thinking: &mut impl FnMut(&str),
    now: impl Fn() -> Instant,
) -> Result<ChatCompletionResult, LlmError> {
    let (tx, rx) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            let result = reader.read(&mut buf).map(|n| StreamReadResult {
                bytes: buf[..n].to_vec(),
                done: n == 0,
            });
            let done = matches!(result, Ok(StreamReadResult { done: true, .. }) | Err(_));
            if tx.send(result).is_err() || done {
                break;
            }
        }
    });

    read_timed_stream_events(rx, format, idle_timeout_secs, on_thinking, now)
}

fn read_timed_stream_events(
    rx: mpsc::Receiver<std::io::Result<StreamReadResult>>,
    format: ThinkingFormat,
    idle_timeout_secs: u64,
    on_thinking: &mut impl FnMut(&str),
    now: impl Fn() -> Instant,
) -> Result<ChatCompletionResult, LlmError> {
    let mut parser = SseParser::new();
    let mut reducer = StreamReducer::new(format);
    let mut last_activity = now();
    let idle_timeout = Duration::from_secs(idle_timeout_secs);
    let mut done = false;

    while !done {
        let remaining = idle_timeout
            .checked_sub(now().duration_since(last_activity))
            .unwrap_or(Duration::ZERO);
        if remaining.is_zero() {
            return Err(LlmError::Http(format!(
                "stream idle timeout after {idle_timeout_secs}s"
            )));
        }
        let read_result = rx.recv_timeout(remaining).map_err(|e| match e {
            mpsc::RecvTimeoutError::Timeout => {
                LlmError::Http(format!("stream idle timeout after {idle_timeout_secs}s"))
            }
            mpsc::RecvTimeoutError::Disconnected => {
                LlmError::Http("stream ended before [DONE]".into())
            }
        })?;
        let read = read_result.map_err(|e| {
            if is_timeout_err(&e) {
                LlmError::Http(format!("stream idle timeout after {idle_timeout_secs}s"))
            } else {
                LlmError::Http(e.to_string())
            }
        })?;
        if read.done {
            break;
        }
        last_activity = process_stream_bytes(
            &mut parser,
            &mut reducer,
            &read.bytes,
            on_thinking,
            &now,
            &mut done,
        )?
        .unwrap_or(last_activity);
    }

    if !done {
        return Err(LlmError::Http("stream ended before [DONE]".into()));
    }

    if let Some(event) = parser.flush()
        && event.data != "[DONE]"
    {
        let chunk = serde_json::from_str::<ChatChunk>(&event.data)
            .map_err(|e| LlmError::Decode(e.to_string()))?;
        reducer.apply_chunk(chunk, on_thinking);
    }
    Ok(reducer.finish(on_thinking))
}

pub struct LlmClient {
    base_url: String,
    api_key: Option<String>,
    agent: ureq::Agent,
}

impl LlmClient {
    /// Create a new LLM client from runtime config.
    ///
    /// Uses a ureq Agent with a global timeout (ureq v3 requires agent-level
    /// timeout configuration, not per-request).
    pub fn from_config(config: &RuntimeConfig) -> Self {
        let base_url = config
            .api_base_url
            .clone()
            .unwrap_or_else(|| DEFAULT_OPENAI_BASE.to_string())
            .trim_end_matches('/')
            .to_string();

        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(TIMEOUT_SECS)))
            .build()
            .into();

        Self {
            base_url,
            api_key: config.api_key.clone(),
            agent,
        }
    }

    /// Create a client for a specific model, resolving the correct provider
    /// base URL and API key. The API key may be `None` for local servers.
    pub fn for_model(model: &str) -> Option<Self> {
        let config = provider::provider_config(model);
        let api_key = provider::api_key_for_model(model);
        let base_url = config
            .base_url
            .unwrap_or_else(|| DEFAULT_OPENAI_BASE.to_string())
            .trim_end_matches('/')
            .to_string();

        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(TIMEOUT_SECS)))
            .build()
            .into();

        Some(Self {
            base_url,
            api_key,
            agent,
        })
    }

    pub fn supports_thinking_stream(&self, model: &str) -> bool {
        provider::thinking_format_for(model, &self.base_url).is_some()
    }

    /// Send a chat completion request with a single user message.
    pub fn chat_completion(
        &self,
        model: &str,
        prompt: &str,
        temperature: Option<f64>,
        max_tokens: Option<u64>,
    ) -> Result<ChatCompletionResult, LlmError> {
        self.chat_completion_structured(model, None, prompt, temperature, max_tokens, None)
    }

    pub fn chat_completion_with_thinking(
        &self,
        model: &str,
        prompt: &str,
        temperature: Option<f64>,
        max_tokens: Option<u64>,
        mut on_reset: impl FnMut(),
        mut on_thinking: impl FnMut(&str),
    ) -> Result<ChatCompletionResult, LlmError> {
        let Some(format) = provider::thinking_format_for(model, &self.base_url) else {
            return self.chat_completion(model, prompt, temperature, max_tokens);
        };
        on_reset();

        let body = ChatRequest {
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
        };
        let response = self.send_chat_request(body, true)?;
        let result = read_stream_completion_with_idle_timeout(
            response.into_parts().1.into_reader(),
            format,
            STREAM_READ_TIMEOUT_SECS,
            &mut on_thinking,
            Instant::now,
        )?;
        if result.content.is_empty() {
            return Err(LlmError::Decode("no content in response".into()));
        }
        Ok(result)
    }

    /// Send a chat completion request with optional system message and structured output.
    pub fn chat_completion_structured(
        &self,
        model: &str,
        system_prompt: Option<&str>,
        user_prompt: &str,
        temperature: Option<f64>,
        max_tokens: Option<u64>,
        response_format: Option<&ResponseFormat>,
    ) -> Result<ChatCompletionResult, LlmError> {
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

        let body = ChatRequest {
            model,
            messages,
            temperature,
            max_tokens,
            response_format,
            stream: None,
            stream_options: None,
            extra_body: None,
        };

        let mut response = self.send_chat_request(body, false)?;
        let resp: ChatResponse = response
            .body_mut()
            .read_json()
            .map_err(|e| LlmError::Decode(e.to_string()))?;

        let content = resp
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .ok_or_else(|| LlmError::Decode("no content in response".into()))?;

        Ok(ChatCompletionResult {
            content,
            usage: resp.usage.map(effective_usage),
        })
    }

    fn send_chat_request(
        &self,
        body: ChatRequest<'_>,
        streaming: bool,
    ) -> Result<ureq::http::Response<ureq::Body>, LlmError> {
        let url = format!("{}/chat/completions", self.base_url);
        let mut request = self
            .agent
            .post(&url)
            .header("Content-Type", "application/json");

        if streaming {
            request = request
                .config()
                .timeout_global(None)
                .http_status_as_error(false)
                .build();
        }

        // Only send Authorization header when we have an API key.
        // Local servers (Ollama, llama.cpp) often reject unexpected auth headers.
        if let Some(ref key) = self.api_key {
            request = request.header("Authorization", &format!("Bearer {key}"));
        }

        let body = serde_json::to_vec(&body).map_err(|e| LlmError::Decode(e.to_string()))?;
        let mut response = request.send(&body[..]).map_err(|e| match e {
            // 429 and 5xx are transient; other 4xx are permanent (bad key,
            // invalid model, malformed request) and should not be retried.
            ureq::Error::StatusCode(429) => LlmError::Http("HTTP 429: rate limited".to_string()),
            ureq::Error::StatusCode(code) if code >= 500 => {
                LlmError::Http(format!("HTTP {code}: server error"))
            }
            ureq::Error::StatusCode(code) => {
                LlmError::Api(format!("HTTP {code}: non-retryable error"))
            }
            other => LlmError::Http(other.to_string()),
        })?;

        if streaming && !response.status().is_success() {
            let status = response.status();
            let body = response.body_mut().read_to_string().unwrap_or_default();
            if status.as_u16() == 429 || status.is_server_error() {
                return Err(LlmError::Http(format!("HTTP {status}: {body}")));
            }
            return Err(LlmError::Api(format!("HTTP {status}: {body}")));
        }

        Ok(response)
    }
}

fn is_timeout_err(e: &std::io::Error) -> bool {
    if e.kind() == std::io::ErrorKind::TimedOut {
        return true;
    }
    let s = e.to_string();
    s.contains("timeout") || s.contains("Timeout")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::io;

    fn chunk(json: serde_json::Value) -> ChatChunk {
        serde_json::from_value(json).unwrap()
    }

    struct ChunkReader {
        chunks: Vec<&'static [u8]>,
        idx: usize,
    }

    impl ChunkReader {
        fn new(chunks: Vec<&'static [u8]>) -> Self {
            Self { chunks, idx: 0 }
        }
    }

    impl Read for ChunkReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let Some(chunk) = self.chunks.get(self.idx) else {
                return Ok(0);
            };
            self.idx += 1;
            buf[..chunk.len()].copy_from_slice(chunk);
            Ok(chunk.len())
        }
    }

    #[test]
    fn stream_reducer_separates_reasoning_content() {
        let mut reducer = StreamReducer::new(ThinkingFormat::ReasoningContent);
        let mut thinking = String::new();
        reducer.apply_chunk(
            chunk(serde_json::json!({
                "choices": [{ "delta": { "reasoning_content": "think ", "content": "[" } }]
            })),
            &mut |delta| thinking.push_str(delta),
        );
        reducer.apply_chunk(
            chunk(serde_json::json!({
                "choices": [{ "delta": { "reasoning_content": "more", "content": "{}]" } }],
                "usage": { "prompt_tokens": 10, "completion_tokens": 2 }
            })),
            &mut |delta| thinking.push_str(delta),
        );
        let result = reducer.finish(&mut |delta| thinking.push_str(delta));
        assert_eq!(thinking, "think more");
        assert_eq!(result.content, "[{}]");
        assert_eq!(result.usage.unwrap().completion_tokens, 2);
    }

    #[test]
    fn tag_splitter_handles_split_gemini_thoughts() {
        let mut reducer = StreamReducer::new(ThinkingFormat::GeminiThoughtTags);
        let mut thinking = String::new();
        for content in ["<tho", "ught>plan", "</thou", "ght>[{}]"] {
            reducer.apply_chunk(
                chunk(serde_json::json!({
                    "choices": [{ "delta": { "content": content } }]
                })),
                &mut |delta| thinking.push_str(delta),
            );
        }
        let result = reducer.finish(&mut |delta| thinking.push_str(delta));
        assert_eq!(thinking, "plan");
        assert_eq!(result.content, "[{}]");
    }

    #[test]
    fn tag_splitter_preserves_literal_tag_text_outside_tags() {
        let mut reducer = StreamReducer::new(ThinkingFormat::GeminiThoughtTags);
        let mut thinking = String::new();
        reducer.apply_chunk(
            chunk(serde_json::json!({
                "choices": [{ "delta": { "content": "[{\"Front\":\"literal <thought" } }]
            })),
            &mut |delta| thinking.push_str(delta),
        );
        reducer.apply_chunk(
            chunk(serde_json::json!({
                "choices": [{ "delta": { "content": " tag\"}]" } }]
            })),
            &mut |delta| thinking.push_str(delta),
        );
        let result = reducer.finish(&mut |delta| thinking.push_str(delta));
        assert!(thinking.is_empty());
        assert_eq!(result.content, "[{\"Front\":\"literal <thought tag\"}]");
    }

    #[test]
    fn tag_splitter_preserves_literal_full_thought_tag_inside_json() {
        let mut reducer = StreamReducer::new(ThinkingFormat::GeminiThoughtTags);
        let mut thinking = String::new();
        reducer.apply_chunk(
            chunk(serde_json::json!({
                "choices": [{ "delta": { "content": "[{\"Front\":\"literal <thought>tag</thought> text\"}]" } }]
            })),
            &mut |delta| thinking.push_str(delta),
        );
        let result = reducer.finish(&mut |delta| thinking.push_str(delta));
        assert!(thinking.is_empty());
        assert_eq!(
            result.content,
            "[{\"Front\":\"literal <thought>tag</thought> text\"}]"
        );
    }

    #[test]
    fn streamed_reasoning_content_counts_as_activity() {
        let reader = ChunkReader::new(vec![
            br#"data: {"choices":[{"delta":{"reasoning_content":"think"}}]}

"#,
            br#"data: {"choices":[{"delta":{"reasoning_content":" more"}}]}

"#,
            br#"data: {"choices":[{"delta":{"content":"[{}]"}}]}

"#,
            b"data: [DONE]\n\n",
        ]);
        let elapsed = Cell::new(Instant::now());
        let mut thinking = String::new();

        let result = read_stream_completion(
            reader,
            ThinkingFormat::ReasoningContent,
            30,
            &mut |delta| thinking.push_str(delta),
            || {
                let next = elapsed.get() + Duration::from_secs(20);
                elapsed.set(next);
                next
            },
        )
        .unwrap();

        assert_eq!(thinking, "think more");
        assert_eq!(result.content, "[{}]");
    }

    #[test]
    fn stream_without_sse_events_hits_idle_timeout() {
        let reader = ChunkReader::new(vec![b": keepalive\n", b": still no event\n"]);
        let elapsed = Cell::new(Instant::now());
        let result = read_stream_completion(
            reader,
            ThinkingFormat::ReasoningContent,
            30,
            &mut |_| {},
            || {
                let next = elapsed.get() + Duration::from_secs(20);
                elapsed.set(next);
                next
            },
        );

        assert!(matches!(
            result,
            Err(LlmError::Http(message)) if message == "stream idle timeout after 30s"
        ));
    }

    #[test]
    fn effective_usage_includes_hidden_total_tokens() {
        let usage = effective_usage(ChatUsage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: Some(30),
            completion_tokens_details: None,
        });
        assert_eq!(usage.completion_tokens, 20);
    }

    #[test]
    fn effective_usage_includes_reasoning_token_details() {
        let usage = effective_usage(ChatUsage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: None,
            completion_tokens_details: Some(CompletionTokensDetails {
                reasoning_tokens: Some(12),
            }),
        });
        assert_eq!(usage.completion_tokens, 12);
    }
}

use std::io::Read;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;

use super::response::{ChatCompletionResult, ChatUsage, effective_usage};
use crate::llm::error::LlmError;
use crate::llm::provider::ThinkingFormat;
use crate::llm::sse::SseParser;

#[derive(Deserialize)]
pub(super) struct ChatChunk {
    #[serde(default)]
    pub choices: Vec<ChatChunkChoice>,
    pub usage: Option<ChatUsage>,
}

#[derive(Deserialize)]
pub(super) struct ChatChunkChoice {
    pub delta: ChatDelta,
}

#[derive(Deserialize, Default)]
pub(super) struct ChatDelta {
    pub content: Option<String>,
    #[serde(default)]
    pub reasoning_content: Option<String>,
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

pub(super) struct StreamReducer {
    splitter: Option<TagSplitter>,
    content: String,
    usage: Option<ChatUsage>,
}

impl StreamReducer {
    pub(super) fn new(format: ThinkingFormat) -> Self {
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

    pub(super) fn apply_chunk(&mut self, chunk: ChatChunk, on_thinking: &mut impl FnMut(&str)) {
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

    pub(super) fn finish(mut self, on_thinking: &mut impl FnMut(&str)) -> ChatCompletionResult {
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

fn is_timeout_err(e: &std::io::Error) -> bool {
    if e.kind() == std::io::ErrorKind::TimedOut {
        return true;
    }
    let s = e.to_string();
    s.contains("timeout") || s.contains("Timeout")
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

pub(super) fn read_stream_completion_with_idle_timeout<R: Read + Send + 'static>(
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
}

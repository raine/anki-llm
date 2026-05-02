use std::io::Read;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use super::super::response::ChatCompletionResult;
use super::reducer::StreamReducer;
use super::wire::ChatChunk;
use crate::llm::error::LlmError;
use crate::llm::provider::ThinkingFormat;
use crate::llm::sse::SseParser;

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

    finish_stream(parser, reducer, done, on_thinking)
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

pub(in crate::llm::client) fn read_stream_completion_with_idle_timeout<R: Read + Send + 'static>(
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

    finish_stream(parser, reducer, done, on_thinking)
}

fn finish_stream(
    parser: SseParser,
    mut reducer: StreamReducer,
    done: bool,
    on_thinking: &mut impl FnMut(&str),
) -> Result<ChatCompletionResult, LlmError> {
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
    fn threaded_reader_reads_valid_stream() {
        let reader = ChunkReader::new(vec![
            br#"data: {"choices":[{"delta":{"reasoning_content":"think"}}]}

"#,
            br#"data: {"choices":[{"delta":{"content":"[{}]"}}]}

"#,
            b"data: [DONE]\n\n",
        ]);
        let mut thinking = String::new();

        let result = read_stream_completion_with_idle_timeout(
            reader,
            ThinkingFormat::ReasoningContent,
            30,
            &mut |delta| thinking.push_str(delta),
            Instant::now,
        )
        .unwrap();

        assert_eq!(thinking, "think");
        assert_eq!(result.content, "[{}]");
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
    fn stream_ending_before_done_is_error() {
        let reader = ChunkReader::new(vec![
            br#"data: {"choices":[{"delta":{"content":"[]"}}]}

"#,
        ]);

        let result = read_stream_completion(
            reader,
            ThinkingFormat::ReasoningContent,
            30,
            &mut |_| {},
            Instant::now,
        );

        assert!(matches!(
            result,
            Err(LlmError::Http(message)) if message == "stream ended before [DONE]"
        ));
    }
}

use super::super::response::{ChatCompletionResult, ChatUsage, effective_usage};
use super::tag_splitter::{Segment, TagSplitter};
use super::wire::ChatChunk;
use crate::llm::provider::ThinkingFormat;

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

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(json: serde_json::Value) -> ChatChunk {
        serde_json::from_value(json).unwrap()
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
    fn stream_reducer_normalizes_streamed_usage() {
        let mut reducer = StreamReducer::new(ThinkingFormat::ReasoningContent);
        reducer.apply_chunk(
            chunk(serde_json::json!({
                "choices": [{ "delta": { "content": "[]" } }],
                "usage": {
                    "prompt_tokens": 10,
                    "completion_tokens": 2,
                    "total_tokens": 40,
                    "completion_tokens_details": { "reasoning_tokens": 25 }
                }
            })),
            &mut |_| {},
        );

        let result = reducer.finish(&mut |_| {});

        assert_eq!(result.usage.unwrap().completion_tokens, 30);
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
}

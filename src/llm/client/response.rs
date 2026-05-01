use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct ChatResponse {
    pub choices: Vec<ChatChoice>,
    pub usage: Option<ChatUsage>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ChatChoice {
    pub message: ChatChoiceMessage,
}

#[derive(Debug, Deserialize)]
pub(super) struct ChatChoiceMessage {
    pub content: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    #[serde(default)]
    pub(super) total_tokens: Option<u64>,
    #[serde(default)]
    pub(super) completion_tokens_details: Option<CompletionTokensDetails>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct CompletionTokensDetails {
    #[serde(default)]
    pub reasoning_tokens: Option<u64>,
}

/// Result of a chat completion call.
pub struct ChatCompletionResult {
    pub content: String,
    pub usage: Option<ChatUsage>,
}

pub(super) fn effective_usage(mut usage: ChatUsage) -> ChatUsage {
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
mod tests {
    use super::*;

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

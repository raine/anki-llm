use serde::Deserialize;

use super::super::response::ChatUsage;

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

use std::path::PathBuf;

use super::super::cards::ValidatedCard;
use super::super::pipeline::PipelineStep;
use super::super::process::FlaggedCard;

pub struct SessionInfo {
    pub deck: String,
    pub note_type: String,
    pub model: String,
    pub available_models: Vec<String>,
    pub field_map: indexmap::IndexMap<String, String>,
    /// Whether the current session has a `tts:` block AND a valid audio
    /// backend was found at startup. When false, the TUI hides the
    /// preview keybind.
    pub tts_preview_enabled: bool,
}

/// Per-card TTS preview state, routed by stable `card_id`. The TUI owns
/// a `HashMap<u64, TtsUiState>` and draws a badge reflecting whichever
/// state the focused card is in.
#[derive(Debug, Clone)]
pub enum TtsUiState {
    Synthesizing,
    Ready { cache_path: PathBuf },
    Failed(String),
}

pub enum BackendEvent {
    SessionReady(SessionInfo),
    Log(String),
    StepUpdate {
        step: PipelineStep,
        status: StepStatus,
    },
    RequestSelection(Vec<ValidatedCard>),
    AppendCards(Vec<ValidatedCard>), // refresh: new unique cards to append
    /// Single-card regeneration result. `previous_card_id` is the
    /// stable id of the card that was sent for regeneration; the TUI
    /// looks up its current row by id and replaces it. If the user has
    /// removed or edited the card meanwhile, the reply is dropped.
    ReplaceCard {
        previous_card_id: u64,
        card: ValidatedCard,
    },
    RegenError(String), // single-card regeneration failed
    RequestReview(Vec<FlaggedCard>),
    CostUpdate {
        input_tokens: u64,
        output_tokens: u64,
        cost: f64,
    },
    /// Per-card TTS preview state update. Routed by `card_id` so the
    /// TUI can map it to the correct selection-screen row even after
    /// regeneration moves indices.
    TtsState {
        card_id: u64,
        state: TtsUiState,
    },
    RunDone {
        message: String,
        cards: Vec<ValidatedCard>,
        /// Anki note IDs of imported cards (empty for exports/dry runs).
        note_ids: Vec<i64>,
    },
    RunError(String),         // single run failed (can retry with new term)
    ModelChangeError(String), // model switch failed
    Fatal(String),            // session-level error (must exit)
}

pub enum WorkerCommand {
    Start(String),           // term to generate cards for
    Refresh,                 // generate more cards for the same term
    RefreshWithTerm(String), // generate more cards with a different term
    /// Regenerate a single card with feedback. The card payload is the
    /// TUI's current snapshot, so any local edits are picked up by the
    /// LLM. `card.card_id` is the routing key for the eventual
    /// `BackendEvent::ReplaceCard` reply.
    RegenerateCard {
        card: ValidatedCard,
        feedback: String,
    },
    /// Synthesize TTS preview audio for a card. The card payload is
    /// the TUI's current snapshot — never a stale worker-side copy —
    /// so synthesis always uses the user's most recent edits.
    PreviewTts {
        card: ValidatedCard,
    },
    /// User confirmed selection. Carries the actual cards in visible
    /// order (with edits, removes, and force-toggled duplicates
    /// applied), not indices into a worker-side mirror.
    Selection(Vec<ValidatedCard>),
    Review(Vec<bool>), // true = keep, false = discard
    SetModel(String),  // change model between runs
    Cancel,            // abandon current run, go back to input
    Quit,
}

#[derive(Clone, PartialEq)]
pub enum StepStatus {
    Pending,
    Running(Option<String>),
    Done(Option<String>),
    Skipped,
    Error(String),
}

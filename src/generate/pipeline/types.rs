use crate::anki::client::AnkiClient;
use crate::llm::client::LlmClient;
use crate::llm::logger::LlmLogger;
use crate::template::frontmatter::Frontmatter;

use super::super::cards::ValidatedCard;
use super::super::process::FlaggedCard;
use super::super::validate::ValidationResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineStep {
    LoadPrompt,
    ValidateAnki,
    Generate,
    PostProcess,
    Validate,
    Select,
    QualityCheck,
    FinalizeTts,
    Finish,
    /// UI-only terminal step. The pipeline never emits events for it;
    /// the TUI marks it Done when the run reaches `RunDone` and routes
    /// the run summary view to this step's sidebar entry.
    Summary,
}

impl PipelineStep {
    pub fn label(self) -> &'static str {
        match self {
            Self::LoadPrompt => "Load prompt",
            Self::ValidateAnki => "Validate Anki",
            Self::Generate => "Generate cards",
            Self::PostProcess => "Pre-select processing",
            Self::Validate => "Check duplicates",
            Self::Select => "Select cards",
            Self::QualityCheck => "Post-select processing",
            Self::FinalizeTts => "Synthesize audio",
            Self::Finish => "Import / export",
            Self::Summary => "Summary",
        }
    }
}

pub trait PipelineProgress: Send + Sync {
    fn log(&self, msg: &str);
    fn step_start(&self, step: PipelineStep, detail: Option<&str>);
    fn step_done(&self, step: PipelineStep, detail: Option<String>);
    fn step_skip(&self, step: PipelineStep);
    fn step_error(&self, step: PipelineStep, detail: &str);
    fn cost_update(&self, input_tokens: u64, output_tokens: u64, cost: f64);
    fn thinking_reset(&self) {}
    fn thinking_delta(&self, _delta: &str) {}
    fn thinking_clear(&self) {}
}

pub enum SelectionAction {
    Selected {
        cards: Vec<ValidatedCard>,
        skip_post_select: bool,
    },
    Refresh,
    RefreshWithTerm(String),
    RegenerateCard {
        card: ValidatedCard,
        feedback: String,
    },
    PreviewTts {
        card: ValidatedCard,
    },
    Cancel,
    Quit,
}

pub enum ReviewResult {
    Reviewed(Vec<bool>),
    Cancel,
}

pub trait PipelineInteraction {
    fn begin_selection(&self, cards: Vec<ValidatedCard>);
    fn append_selection(&self, cards: Vec<ValidatedCard>);
    fn replace_card(&self, previous_card_id: u64, card: ValidatedCard);
    fn regen_error(&self, target_id: u64, message: String);
    fn wait_selection(&self) -> SelectionAction;
    fn request_review(&self, flagged: Vec<FlaggedCard>) -> ReviewResult;
    /// Announce a TTS preview state transition for a given card id.
    /// Default impl is a no-op so legacy / copy mode can ignore it.
    fn tts_state(&self, _card_id: u64, _state: super::super::tui::events::TtsUiState) {}
}

pub struct PipelineConfig<'a> {
    pub frontmatter: &'a Frontmatter,
    pub prompt_body: &'a str,
    pub field_map_keys: &'a [String],
    pub validation: &'a ValidationResult,
    pub client: &'a LlmClient,
    pub anki: &'a AnkiClient,
    pub logger: &'a LlmLogger,
    pub model: &'a str,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u64>,
    pub retries: u32,
    pub count: u32,
    pub dry_run: bool,
    pub output: Option<&'a std::path::Path>,
    /// Optional lazy TTS handle. When present, the preview and import
    /// paths resolve the underlying `TtsBundle` via
    /// `SessionTts::bundle()` on first use; neither `--dry-run` nor
    /// `--output` touch this field, so TTS credential resolution is
    /// deferred (or skipped) for those flows.
    pub tts: Option<&'a crate::tts::service::SessionTts>,
    pub enable_thinking_stream: bool,
}

pub enum PipelineOutcome {
    Success {
        message: String,
        cards: Vec<ValidatedCard>,
        note_ids: Vec<i64>,
        /// When true, the run finished with a non-fatal failure and the
        /// message should be rendered in an error style. The cards are
        /// still returned so the user can recover them (copy-to-clipboard
        /// from the Done view). Used to preserve user-curated state when
        /// late-stage steps like `finalize_tts` fail transiently.
        failed: bool,
    },
    Cancelled,
    Quit,
}

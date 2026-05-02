use std::sync::mpsc;

use super::events::{BackendEvent, SessionInfo, StepStatus, WorkerCommand};
use super::history::InputHistory;
use super::screens::review::ReviewState;
use super::screens::selection::SelectionState;
use super::widgets::ModelPickerState;

use crate::generate::cards::ValidatedCard;
use crate::generate::pipeline::PipelineStep;
use crate::tui::line_input::LineInput;
use crate::tui::theme::Glyphs;

pub(super) const MAX_THINKING_CHARS: usize = 12_000;

pub(super) const ALL_STEPS: &[PipelineStep] = &[
    PipelineStep::LoadPrompt,
    PipelineStep::ValidateAnki,
    PipelineStep::Generate,
    PipelineStep::PostProcess,
    PipelineStep::Validate,
    PipelineStep::Select,
    PipelineStep::QualityCheck,
    PipelineStep::FinalizeTts,
    PipelineStep::Finish,
    PipelineStep::Summary,
];

pub(super) fn summary_step_idx() -> usize {
    ALL_STEPS
        .iter()
        .position(|s| matches!(s, PipelineStep::Summary))
        .expect("Summary step must be in ALL_STEPS")
}

pub(super) struct StepRecord {
    pub(super) step: PipelineStep,
    pub(super) status: StepStatus,
    pub(super) logs: Vec<String>,
}

pub(super) enum AppMode {
    Input(LineInput), // term text being typed
    Running,
    Selecting(SelectionState),
    Reviewing(ReviewState),
    Done {
        message: String,
        cards: Vec<ValidatedCard>,
        note_ids: Vec<i64>,
        /// When true, the run finished with a non-fatal failure and the
        /// summary header should render in an error style. Cards are
        /// still shown so the user can copy out work they had curated
        /// before the failure.
        failed: bool,
    },
    Error(String),
}

pub(super) struct App {
    pub(super) mode: AppMode,
    pub(super) session_info: Option<SessionInfo>,
    pub(super) logs: Vec<String>,
    pub(super) steps: Vec<StepRecord>,
    /// Index of the currently-running step (for bucketing logs).
    pub(super) current_step_idx: Option<usize>,
    /// Cost for the current run.
    pub(super) run_cost: f64,
    pub(super) run_input_tokens: u64,
    pub(super) run_output_tokens: u64,
    /// Accumulated cost across all runs in this session.
    pub(super) session_cost: f64,
    pub(super) log_scroll: u16,
    pub(super) log_auto_scroll: bool,
    pub(super) thinking: String,
    pub(super) tick: u64,
    /// Counts how many runs have been cancelled. While > 0, backend events are
    /// discarded. Decremented when RunDone/RunError arrives from a cancelled run.
    pub(super) pending_cancels: u32,
    pub(super) should_quit: bool,
    /// True when the user explicitly pressed q/Ctrl-C (as opposed to natural Done/Error exit).
    pub(super) user_quit: bool,
    /// True when the user pressed Ctrl+P to switch prompt.
    pub(super) switch_prompt: bool,
    pub(super) show_help: bool,
    pub(super) model_picker: Option<ModelPickerState>,
    /// Last term submitted, for retry.
    pub(super) last_term: Option<String>,
    /// Model name to apply before the next pipeline run (deferred model change).
    pub(super) pending_model: Option<String>,
    /// True after a Fatal error — worker is dead, no new runs possible.
    pub(super) is_fatal: bool,
    pub(super) glyphs: Glyphs,
    pub(super) history: InputHistory,
    pub(super) toast: Option<Toast>,
    /// In Done/Error mode: selected step index for log browsing, None = summary.
    pub(super) browse_step: Option<usize>,
    pub(super) browse_scroll: u16,
    /// Remaining terms to process in a batch (front = next term).
    pub(super) batch_queue: Vec<String>,
    /// Batch progress: (current 1-based index, total count). None when not in batch.
    pub(super) batch_progress: Option<(usize, usize)>,
    /// Accumulated cards during batch processing (before entering selection).
    pub(super) batch_cards: Vec<ValidatedCard>,
    pub(super) backend_rx: mpsc::Receiver<BackendEvent>,
    pub(super) worker_tx: mpsc::SyncSender<WorkerCommand>,
    /// Audio playback thread handle. `Some` when a system player was
    /// detected at session startup AND the prompt has a `tts:` block;
    /// the preview keybind is hidden and ignored when `None`.
    pub(super) player: Option<crate::audio::PlayerHandle>,
    /// Remembered binary discovered at startup. Retained so the player
    /// could be lazily respawned later if needed; currently unused but
    /// kept alongside the handle for symmetry.
    pub(super) player_binary: Option<crate::audio::PlayerBinary>,
}

pub(super) struct Toast {
    pub(super) message: String,
    pub(super) tick: u64,
}

impl App {
    pub(super) fn new(
        initial_term: Option<String>,
        glyphs: Glyphs,
        backend_rx: mpsc::Receiver<BackendEvent>,
        worker_tx: mpsc::SyncSender<WorkerCommand>,
    ) -> Self {
        let steps = ALL_STEPS
            .iter()
            .map(|&s| StepRecord {
                step: s,
                status: StepStatus::Pending,
                logs: Vec::new(),
            })
            .collect();
        let last_term = initial_term.clone();
        let mode = if let Some(term) = initial_term {
            worker_tx
                .send(WorkerCommand::Start {
                    term,
                    enable_thinking_stream: true,
                })
                .ok();
            AppMode::Running
        } else {
            AppMode::Input(LineInput::default())
        };
        App {
            mode,
            session_info: None,
            logs: Vec::new(),
            steps,
            current_step_idx: None,
            run_cost: 0.0,
            run_input_tokens: 0,
            run_output_tokens: 0,
            session_cost: 0.0,
            log_scroll: 0,
            log_auto_scroll: true,
            thinking: String::new(),
            tick: 0,
            pending_cancels: 0,
            should_quit: false,
            user_quit: false,
            switch_prompt: false,
            show_help: false,
            model_picker: None,
            last_term,
            pending_model: None,
            is_fatal: false,
            glyphs,
            history: InputHistory::load(),
            toast: None,
            browse_step: None,
            browse_scroll: 0,
            batch_queue: Vec::new(),
            batch_progress: None,
            batch_cards: Vec::new(),
            backend_rx,
            worker_tx,
            player: None,
            player_binary: crate::audio::detect_player_binary(),
        }
    }

    pub(super) fn reset_for_new_run(&mut self) {
        self.logs.clear();
        self.log_scroll = 0;
        self.log_auto_scroll = true;
        self.thinking.clear();
        self.session_cost += self.run_cost;
        self.run_cost = 0.0;
        self.run_input_tokens = 0;
        self.run_output_tokens = 0;
        for record in &mut self.steps {
            record.status = StepStatus::Pending;
            record.logs.clear();
        }
        self.current_step_idx = None;
        self.browse_step = None;
        self.browse_scroll = 0;
    }

    pub(super) fn copy_cards(&mut self, cards: &[ValidatedCard]) {
        if cards.is_empty() {
            return;
        }
        let text = cards
            .iter()
            .map(|card| {
                card.raw_anki_fields
                    .iter()
                    .map(|(name, value)| {
                        let plain = crate::generate::selector::strip_html_tags(value);
                        format!("{name}\n{plain}")
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n")
            })
            .collect::<Vec<_>>()
            .join("\n\n────────────────────────────────────────\n\n");
        if let Ok(mut cb) = arboard::Clipboard::new() {
            cb.set_text(text).ok();
        }
        self.toast = Some(Toast {
            message: "Copied!".into(),
            tick: self.tick,
        });
    }

    pub(super) fn open_model_picker(&mut self) {
        if let Some(info) = &self.session_info
            && !info.available_models.is_empty()
        {
            self.model_picker = Some(ModelPickerState::new(
                info.available_models.clone(),
                Some(info.model.as_str()),
            ));
        }
    }

    pub(super) fn step_index(&self, step: PipelineStep) -> Option<usize> {
        self.steps.iter().position(|r| r.step == step)
    }

    pub(super) fn step_status_mut(&mut self, step: PipelineStep) -> Option<&mut StepStatus> {
        self.steps
            .iter_mut()
            .find(|r| r.step == step)
            .map(|r| &mut r.status)
    }

    pub(super) fn append_thinking(&mut self, delta: &str) {
        self.thinking.push_str(delta);
        if self.thinking.len() > MAX_THINKING_CHARS {
            let keep_from = self.thinking.len() - MAX_THINKING_CHARS;
            let keep_from = self
                .thinking
                .char_indices()
                .find_map(|(idx, _)| (idx >= keep_from).then_some(idx))
                .unwrap_or(self.thinking.len());
            self.thinking.drain(..keep_from);
        }
    }
}

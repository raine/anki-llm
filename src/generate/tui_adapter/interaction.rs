use std::sync::mpsc;

use super::super::cards::ValidatedCard;
use super::super::pipeline::{PipelineInteraction, ReviewResult, SelectionAction, TtsPreviewState};
use super::super::process::FlaggedCard;
use super::super::tui::{BackendEvent, TtsUiState, WorkerCommand};

pub(super) struct TuiInteraction<'a> {
    pub tx: mpsc::Sender<BackendEvent>,
    pub rx: &'a mpsc::Receiver<WorkerCommand>,
}

impl PipelineInteraction for TuiInteraction<'_> {
    fn begin_selection(&self, cards: Vec<ValidatedCard>) {
        self.tx.send(BackendEvent::RequestSelection(cards)).ok();
    }

    fn append_selection(&self, cards: Vec<ValidatedCard>) {
        self.tx.send(BackendEvent::AppendCards(cards)).ok();
    }

    fn replace_card(&self, previous_card_id: u64, card: ValidatedCard) {
        self.tx
            .send(BackendEvent::ReplaceCard {
                previous_card_id,
                card,
            })
            .ok();
    }

    fn regen_error(&self, target_id: u64, message: String) {
        self.tx
            .send(BackendEvent::RegenError { target_id, message })
            .ok();
    }

    fn wait_selection(&self) -> SelectionAction {
        match self.rx.recv() {
            Ok(WorkerCommand::Refresh) => SelectionAction::Refresh,
            Ok(WorkerCommand::RefreshWithTerm(term)) => SelectionAction::RefreshWithTerm(term),
            Ok(WorkerCommand::RegenerateCard { card, feedback }) => {
                SelectionAction::RegenerateCard { card, feedback }
            }
            Ok(WorkerCommand::PreviewTts { card }) => SelectionAction::PreviewTts { card },
            Ok(WorkerCommand::Selection {
                cards,
                skip_post_select,
            }) => SelectionAction::Selected {
                cards,
                skip_post_select,
            },
            Ok(WorkerCommand::Cancel) => SelectionAction::Cancel,
            Ok(WorkerCommand::Quit) | Err(_) => SelectionAction::Quit,
            _ => SelectionAction::Cancel,
        }
    }

    fn request_review(&self, flagged: Vec<FlaggedCard>) -> ReviewResult {
        self.tx.send(BackendEvent::RequestReview(flagged)).ok();
        match self.rx.recv() {
            Ok(WorkerCommand::Review(decisions)) => ReviewResult::Reviewed(decisions),
            _ => ReviewResult::Cancel,
        }
    }

    fn tts_state(&self, card_id: u64, state: TtsPreviewState) {
        let state = match state {
            TtsPreviewState::Synthesizing => TtsUiState::Synthesizing,
            TtsPreviewState::Ready { cache_path } => TtsUiState::Ready { cache_path },
            TtsPreviewState::Failed(message) => TtsUiState::Failed(message),
        };
        self.tx.send(BackendEvent::TtsState { card_id, state }).ok();
    }
}

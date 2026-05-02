use std::path::PathBuf;

use crossterm::event::KeyEvent;

use super::events::{BackendEvent, WorkerCommand};

use crate::generate::cards::ValidatedCard;

#[allow(dead_code)]
pub(super) enum AppEvent {
    Backend(Box<BackendEvent>),
    Player(crate::audio::PlayerEvent),
    Key(KeyEvent),
    Paste(String),
}

#[allow(dead_code)]
pub(super) enum Effect {
    SendWorker(WorkerCommand),
    TrySendWorker(WorkerCommand),
    StartAudioPlayer(crate::audio::PlayerBinary),
    PlayAudio { card_id: u64, path: PathBuf },
    CopyCards(Vec<ValidatedCard>),
    DeleteFromAnki { note_ids: Vec<i64> },
    OpenEditor { card_index: usize },
    Quit,
    SwitchPrompt,
}

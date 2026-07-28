use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use crossterm::event::{self, Event};
use ratatui::DefaultTerminal;

use super::editor::edit_card_in_editor;
use super::effects::Effect;
use super::events::{BackendEvent, TtsUiState, WorkerCommand};
use super::history::InputHistory;
use super::prompt_picker::run_prompt_picker;
use super::render::draw;
use super::screens::selection::SelectionState;
use super::state::{App, AppMode, AudioStatus, Toast};

use crate::cli::GenerateArgs;
use crate::generate::cards::ValidatedCard;
use crate::tui::theme::Glyphs;

enum ExitReason {
    UserQuit,
    NaturalExit,
    SwitchPrompt,
}

struct RuntimeContext {
    backend_rx: mpsc::Receiver<BackendEvent>,
    worker_tx: mpsc::SyncSender<WorkerCommand>,
    player: Option<crate::audio::PlayerHandle>,
    player_binary: Option<crate::audio::PlayerBinary>,
}

pub(super) fn send_initial_start(
    worker_tx: &mpsc::SyncSender<WorkerCommand>,
    initial_term: Option<String>,
) {
    if let Some(term) = initial_term {
        worker_tx
            .send(WorkerCommand::Start {
                term,
                enable_thinking_stream: true,
            })
            .ok();
    }
}

fn execute_effects(
    terminal: &mut DefaultTerminal,
    runtime: &mut RuntimeContext,
    app: &mut App,
    effects: Vec<Effect>,
) -> anyhow::Result<()> {
    for effect in effects {
        match effect {
            Effect::SendWorker(command) => {
                runtime.worker_tx.send(command).ok();
            }
            Effect::TrySendWorker(command) => {
                runtime.worker_tx.try_send(command).ok();
            }
            Effect::TryPreviewTts { card_id, card } => {
                match runtime
                    .worker_tx
                    .try_send(WorkerCommand::PreviewTts { card })
                {
                    Ok(()) => {}
                    Err(mpsc::TrySendError::Full(_)) => {
                        apply_preview_tts_send_failure(app, card_id, false);
                    }
                    Err(mpsc::TrySendError::Disconnected(_)) => {
                        apply_preview_tts_send_failure(app, card_id, true);
                    }
                }
            }
            Effect::StartAudioPlayer => {
                if runtime.player.is_none()
                    && let Some(binary) = runtime.player_binary.clone()
                {
                    runtime.player = Some(crate::audio::spawn_player(binary));
                    app.audio_status = AudioStatus::Ready;
                }
            }
            Effect::PlayAudio { card_id, path } => {
                if let Some(player) = &runtime.player {
                    let _ = player.play(card_id, path);
                }
            }
            Effect::CopyCards(cards) => copy_cards(app, &cards),
            Effect::DeleteFromAnki { note_ids } => {
                let count = note_ids.len();
                let result = crate::anki::client::anki_client().delete_notes(&note_ids);
                apply_delete_from_anki_result(app, count, result);
            }
            Effect::OpenEditor { card_index } => {
                edit_card_in_editor(terminal, app, card_index);
            }
            Effect::Quit => {
                runtime.worker_tx.send(WorkerCommand::Quit).ok();
                app.should_quit = true;
                app.user_quit = true;
            }
            Effect::SwitchPrompt => {
                runtime.worker_tx.send(WorkerCommand::Quit).ok();
                app.should_quit = true;
                app.switch_prompt = true;
            }
        }
    }
    Ok(())
}

pub(super) fn apply_preview_tts_send_failure(app: &mut App, card_id: u64, disconnected: bool) {
    if let AppMode::Selecting(ref mut state) = app.mode {
        state.tts_states.remove(&card_id);
    }
    if disconnected {
        app.mode = AppMode::Error("Worker thread exited unexpectedly".into());
    } else {
        app.toast = Some(Toast {
            message: "Preview queue full — try again".into(),
            tick: app.tick,
        });
    }
}

fn copy_cards(app: &mut App, cards: &[ValidatedCard]) {
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
    app.toast = Some(Toast {
        message: "Copied!".into(),
        tick: app.tick,
    });
}

pub(super) fn apply_delete_from_anki_result<E: std::fmt::Display>(
    app: &mut App,
    count: usize,
    result: Result<(), E>,
) {
    match result {
        Ok(()) => {
            if let AppMode::Done {
                ref mut note_ids,
                ref mut cards,
                ref mut message,
                ..
            } = app.mode
            {
                note_ids.clear();
                cards.clear();
                *message = format!("Deleted {count} note(s) from Anki.");
                app.toast = Some(Toast {
                    message: format!("Deleted {count} note(s)"),
                    tick: app.tick,
                });
            }
        }
        Err(e) => {
            app.toast = Some(Toast {
                message: format!("Delete failed: {e}"),
                tick: app.tick,
            });
        }
    }
}

fn run_app(
    mut terminal: DefaultTerminal,
    initial_term: Option<String>,
    glyphs: Glyphs,
    backend_rx: mpsc::Receiver<BackendEvent>,
    worker_tx: mpsc::SyncSender<WorkerCommand>,
) -> anyhow::Result<ExitReason> {
    let player_binary = crate::audio::detect_player_binary();
    let audio_status = if player_binary.is_some() {
        AudioStatus::Available
    } else {
        AudioStatus::Unavailable
    };
    let mut runtime = RuntimeContext {
        backend_rx,
        worker_tx,
        player: None,
        player_binary,
    };
    let mut app = App::new(
        initial_term.clone(),
        glyphs,
        InputHistory::load(),
        audio_status,
    );
    send_initial_start(&runtime.worker_tx, initial_term);

    loop {
        app.tick = app.tick.wrapping_add(1);
        terminal.draw(|f| draw(f, &app))?;

        // Drain all pending backend events
        loop {
            match runtime.backend_rx.try_recv() {
                Ok(ev) => {
                    let effects = app.handle_backend_event(ev);
                    execute_effects(&mut terminal, &mut runtime, &mut app, effects)?;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    if !matches!(app.mode, AppMode::Done { .. } | AppMode::Error(_)) {
                        app.mode = AppMode::Error("Worker thread exited unexpectedly".to_string());
                    }
                    break;
                }
            }
        }

        // Drain pending player events (spawn failures etc.) and surface
        // them as toasts. Otherwise a failed `binary.spawn` leaves the
        // user staring at "♪ Audio ready" with nothing playing.
        while let Some(player) = &runtime.player {
            match player.try_recv_event() {
                Ok(ev) => app.handle_player_event(ev),
                Err(_) => break,
            }
        }

        if app.should_quit {
            break;
        }

        // Poll for terminal input (50 ms timeout so we don't block backend events)
        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(key) => {
                    let effects = app.handle_key(key);
                    execute_effects(&mut terminal, &mut runtime, &mut app, effects)?;
                }
                Event::Paste(text) => app.handle_paste_input(text),
                _ => {}
            }
        }

        execute_effects(&mut terminal, &mut runtime, &mut app, Vec::new())?;

        if app.should_quit {
            break;
        }
    }

    if app.switch_prompt {
        Ok(ExitReason::SwitchPrompt)
    } else if app.user_quit {
        Ok(ExitReason::UserQuit)
    } else {
        Ok(ExitReason::NaturalExit)
    }
}

pub fn run_tui(mut args: GenerateArgs) -> anyhow::Result<()> {
    use crate::workspace::resolver::{ResolvedPrompt, resolve_prompt, save_last_prompt};

    let mut force_picker = false;
    loop {
        // Resolve prompt before entering the TUI. If multiple prompts are
        // available and none was specified, show an interactive picker.
        if args.prompt.is_none() {
            match resolve_prompt(None, force_picker)? {
                ResolvedPrompt::Resolved(path) => {
                    save_last_prompt(&path);
                    args.prompt = Some(path);
                }
                ResolvedPrompt::ShowPicker(prompts) => {
                    let terminal = ratatui::init();
                    let glyphs = Glyphs::from_config();
                    let result = run_prompt_picker(terminal, &prompts, &glyphs);
                    ratatui::restore();
                    match result {
                        Some(path) => {
                            save_last_prompt(&path);
                            args.prompt = Some(path);
                        }
                        None => return Ok(()), // user cancelled
                    }
                }
            }
        }

        let initial_term = args.term.take(); // only use CLI term on first iteration

        let (tx_events, rx_events) = mpsc::channel::<BackendEvent>();
        let (tx_cmd, rx_cmd) = mpsc::sync_channel::<WorkerCommand>(10);

        let pipeline_args = GenerateArgs {
            prompt: args.prompt.clone(),
            term: initial_term.clone(),
            count: args.count,
            model: args.model.clone(),
            api_base_url: args.api_base_url.clone(),
            api_key: args.api_key.clone(),
            dry_run: args.dry_run,
            retries: args.retries,
            max_tokens: args.max_tokens,
            temperature: args.temperature,
            output: args.output.clone(),
            copy: args.copy,
            log: args.log.clone(),
            very_verbose: args.very_verbose,
        };

        let worker_handle = std::thread::spawn(move || {
            crate::generate::tui_adapter::run_pipeline(pipeline_args, tx_events, rx_cmd)
        });

        let glyphs = Glyphs::from_config();
        let terminal = ratatui::init();
        crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste).ok();
        let exit = run_app(terminal, initial_term, glyphs, rx_events, tx_cmd)
            .unwrap_or(ExitReason::UserQuit);
        crossterm::execute!(std::io::stdout(), crossterm::event::DisableBracketedPaste).ok();
        ratatui::restore();

        match exit {
            ExitReason::SwitchPrompt => {
                args.prompt = None;
                force_picker = true;
                continue;
            }
            ExitReason::UserQuit => {
                std::process::exit(0);
            }
            ExitReason::NaturalExit => {
                return worker_handle
                    .join()
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("Worker thread panicked")));
            }
        }
    }
}

/// True when *any* card in the selection has a TTS preview in flight.
/// Used by the Enter/Esc guards to block terminal actions that would
/// otherwise race the worker's FIFO action queue behind an in-flight
/// `PreviewTts` — issue #9. The optimistic `Synthesizing` state set
/// on the `p` keypress is what makes this guard fire immediately,
/// before the worker's own `TtsState::Synthesizing` reply
/// round-trips.
///
/// The check is selection-global, not focused-row-local: the worker
/// command channel is a shared FIFO, so once *any* card is in
/// `Synthesizing`, any `Selection` or `Cancel` the user sends queues
/// behind that `PreviewTts` and re-opens the race — moving the
/// cursor to a different card doesn't help.
pub(super) fn any_card_synthesizing(state: &SelectionState) -> bool {
    state
        .tts_states
        .values()
        .any(|s| matches!(s, TtsUiState::Synthesizing))
}

pub(super) fn done_audio_cache_path(card: &ValidatedCard) -> Option<PathBuf> {
    let sound_tag = card
        .raw_anki_fields
        .values()
        .find(|value| value.starts_with("[sound:") && value.ends_with(']'))?;
    let filename = sound_tag.strip_prefix("[sound:")?.strip_suffix(']')?.trim();
    if filename.is_empty() {
        return None;
    }
    let cache_dir = crate::tts::cache::TtsCache::default_dir()?;
    let path = cache_dir.join(filename);
    path.is_file().then_some(path)
}

mod draw;
mod state;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;

use super::events::{BatchEvent, BatchPlan};
use state::{AppMode, DoneState, RunState, TuiResult};

pub use state::TuiResult as BatchTuiResult;

/// Run the batch TUI. Shows preflight, then running, then done/error screen.
///
/// The `start_tx` channel is used as a barrier: the engine thread blocks on
/// the receiving end until the user confirms (Enter) on the preflight screen.
/// Dropping `start_tx` (Esc) signals the engine to exit without processing.
pub fn run_tui(
    plan: BatchPlan,
    event_rx: mpsc::Receiver<BatchEvent>,
    cancel: Arc<AtomicBool>,
    start_tx: mpsc::SyncSender<()>,
) -> anyhow::Result<BatchTuiResult> {
    let mut terminal = crate::tui::terminal::init();
    let result = run_app(&mut terminal, plan, event_rx, cancel, start_tx);
    crate::tui::terminal::restore();
    result
}

fn run_app(
    terminal: &mut DefaultTerminal,
    plan: BatchPlan,
    event_rx: mpsc::Receiver<BatchEvent>,
    cancel: Arc<AtomicBool>,
    start_tx: mpsc::SyncSender<()>,
) -> anyhow::Result<BatchTuiResult> {
    let mut mode = AppMode::Preflight;
    let mut start_tx = Some(start_tx);
    let mut should_quit = false;
    let mut retry_requested = false;

    loop {
        terminal.draw(|f| draw::draw(&mode, &plan, f))?;

        // Drain pending batch events (only when running)
        if matches!(mode, AppMode::Running(_)) {
            while let Ok(evt) = event_rx.try_recv() {
                handle_batch_event(&mut mode, evt);
            }
        }

        // Poll crossterm events (50ms for spinner animation)
        if event::poll(Duration::from_millis(50))?
            && let Event::Key(key) = event::read()?
        {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            let is_ctrl_c =
                key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL);
            match &mut mode {
                AppMode::Preflight => {
                    if is_ctrl_c || key.code == KeyCode::Esc {
                        drop(start_tx.take());
                        return Ok(TuiResult::Cancelled);
                    } else if key.code == KeyCode::Enter {
                        if let Some(tx) = start_tx.take() {
                            tx.send(()).ok();
                        }
                        mode = AppMode::Running(RunState::from_plan(&plan));
                    }
                }
                AppMode::Running(state) => {
                    if is_ctrl_c || key.code == KeyCode::Esc || key.code == KeyCode::Char('q') {
                        cancel.store(true, Ordering::SeqCst);
                        state.cancelling = true;
                    } else {
                        match key.code {
                            KeyCode::Char('j') | KeyCode::Down => state.scroll_down(),
                            KeyCode::Char('k') | KeyCode::Up => state.scroll_up(),
                            _ => {}
                        }
                    }
                }
                AppMode::Done(state) => {
                    if is_ctrl_c || key.code == KeyCode::Char('q') || key.code == KeyCode::Esc {
                        should_quit = true;
                    } else if key.code == KeyCode::Char('r')
                        && !state.summary.failed_rows.is_empty()
                    {
                        retry_requested = true;
                        should_quit = true;
                    } else {
                        match key.code {
                            KeyCode::Char('j') | KeyCode::Down => {
                                if state.cursor + 1 < state.summary.failed_rows.len() {
                                    state.cursor += 1;
                                }
                            }
                            KeyCode::Char('k') | KeyCode::Up => {
                                state.cursor = state.cursor.saturating_sub(1);
                            }
                            _ => {}
                        }
                    }
                }
                AppMode::Error(_) => {
                    if is_ctrl_c || matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) {
                        should_quit = true;
                    }
                }
            }
        }

        if should_quit {
            if retry_requested && let AppMode::Done(ref done) = mode {
                let rows = done
                    .summary
                    .failed_rows
                    .iter()
                    .map(|f| f.row_data.clone())
                    .collect();
                return Ok(TuiResult::RetryFailed(rows));
            }
            // If quitting while running, cancel and drain
            if matches!(mode, AppMode::Running(_)) {
                cancel.store(true, Ordering::SeqCst);
                for evt in &event_rx {
                    if matches!(evt, BatchEvent::RunDone(_)) {
                        break;
                    }
                }
                return Ok(TuiResult::Cancelled);
            }
            return Ok(TuiResult::Done);
        }

        // Tick spinner
        if let AppMode::Running(ref mut state) = mode {
            state.tick += 1;
        }
    }
}

fn handle_batch_event(mode: &mut AppMode, event: BatchEvent) {
    let AppMode::Running(state) = mode else {
        return;
    };
    match event {
        BatchEvent::RowStateChanged(update) => {
            state.apply_row_update(update);
        }
        BatchEvent::Log(msg) => {
            state.log.push(msg);
            state.log_scroll = state.log.len().saturating_sub(1) as u16;
        }
        BatchEvent::CostUpdate {
            input_tokens,
            output_tokens,
            cost,
        } => {
            state.stats.input_tokens = input_tokens;
            state.stats.output_tokens = output_tokens;
            state.stats.cost = cost;
        }
        BatchEvent::RunDone(summary) => {
            *mode = AppMode::Done(DoneState { summary, cursor: 0 });
        }
        BatchEvent::Fatal(msg) => {
            *mode = AppMode::Error(msg);
        }
    }
}

mod draw;
pub mod state;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::TryRecvError;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;

use super::events::{BatchEvent, BatchPlan};
pub use state::{AppMode, RunState, TuiResult as BatchTuiResult};
use state::{DoneState, TuiResult};

/// Run the batch TUI. Caller (controller) owns terminal lifecycle.
///
/// `initial_mode` is `Preflight` for the first iteration and `Running(...)`
/// for retry iterations so retries skip the preflight screen.
///
/// `start_tx` is the barrier the engine thread blocks on; pass `Some` when the
/// initial mode is `Preflight`. For `Running`, the controller has already
/// signaled the engine, so pass `None`.
pub fn run_tui(
    terminal: &mut DefaultTerminal,
    plan: &BatchPlan,
    initial_mode: AppMode,
    event_rx: mpsc::Receiver<BatchEvent>,
    cancel: Arc<AtomicBool>,
    start_tx: Option<mpsc::SyncSender<()>>,
) -> anyhow::Result<BatchTuiResult> {
    let mut mode = initial_mode;
    let mut start_tx = start_tx;
    let mut should_quit = false;
    let mut retry_requested = false;

    loop {
        terminal.draw(|f| draw::draw(&mode, plan, f))?;

        // Drain pending batch events (only when running)
        if matches!(mode, AppMode::Running(_)) {
            loop {
                match event_rx.try_recv() {
                    Ok(evt) => handle_batch_event(&mut mode, evt),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        // Worker thread dropped the sender without emitting
                        // RunDone or Fatal — treat as a fatal engine crash
                        // so the user isn't trapped watching a frozen spinner.
                        mode = AppMode::Error(
                            "Engine thread exited unexpectedly without finalizing.".into(),
                        );
                        break;
                    }
                }
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
                        // Drop the start signal so the worker exits without running
                        drop(start_tx.take());
                        return Ok(TuiResult::Cancelled);
                    } else if key.code == KeyCode::Enter {
                        if let Some(tx) = start_tx.take() {
                            tx.send(()).ok();
                        }
                        mode = AppMode::Running(RunState::from_plan(plan));
                    }
                }
                AppMode::Running(state) => {
                    if is_ctrl_c || key.code == KeyCode::Esc || key.code == KeyCode::Char('q') {
                        if state.cancelling {
                            // Force-quit escape hatch — second press while
                            // cancellation is in flight exits the process
                            // immediately. Returning would still block on
                            // engine_handle.join() in the controller, which
                            // is exactly the deadlock we want to escape.
                            crate::tui::terminal::restore();
                            std::process::exit(130);
                        }
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
                    } else if key.code == KeyCode::Char('r') && state.summary.can_retry_failed {
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
            return Ok(TuiResult::Done);
        }

        // Tick spinner
        if let AppMode::Running(ref mut state) = mode {
            state.tick += 1;
        }
    }
}

fn handle_batch_event(mode: &mut AppMode, event: BatchEvent) {
    // RunDone and Fatal need to consume the RunState, so handle them specially
    match &event {
        BatchEvent::RunDone(_) => {
            let BatchEvent::RunDone(summary) = event else {
                unreachable!()
            };
            // Take the RunState out of the current mode
            let prev = std::mem::replace(mode, AppMode::Error(String::new()));
            let mut run = match prev {
                AppMode::Running(run) => run,
                other => {
                    *mode = other;
                    return;
                }
            };
            // Freeze elapsed time so the sidebar stops ticking
            run.stats.frozen_elapsed = Some(run.stats.start_time.elapsed());
            *mode = AppMode::Done(DoneState {
                summary,
                run,
                cursor: 0,
            });
            return;
        }
        BatchEvent::Fatal(_) => {
            let BatchEvent::Fatal(msg) = event else {
                unreachable!()
            };
            *mode = AppMode::Error(msg);
            return;
        }
        _ => {}
    }

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
        BatchEvent::RunDone(_) | BatchEvent::Fatal(_) => unreachable!(),
    }
}

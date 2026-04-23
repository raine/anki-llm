use std::io::IsTerminal;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, mpsc};
use std::thread;

use anyhow::Result;
use ratatui::DefaultTerminal;

use crate::data::Row;

use super::engine::{BatchConfig, run_batch};
use super::events::{BatchEvent, BatchPlan, BatchSummary};
use super::plain::run_plain_renderer;
use super::session::SharedSession;
use super::tui::{AppMode, BatchTuiResult, RunState, run_tui};

/// Processor-agnostic knobs the controller needs to drive `run_batch`.
/// Both LLM and TTS sessions build one of these; the fields that only make
/// sense for LLM sessions are expressed as `Option`s so TTS can leave them
/// unset.
#[derive(Clone)]
pub struct ControllerRuntime {
    pub batch_size: u32,
    pub retries: u32,
    pub model: Option<String>,
}

/// Drives a batch run end-to-end. Owns terminal lifecycle, runs the engine in
/// a worker thread, dispatches `RunDone`/`Fatal` only after the session has
/// finalized the iteration, and supports retry-failed loops.
pub fn run_batch_controller(
    mut plan: BatchPlan,
    runtime: &ControllerRuntime,
    pending_rows: Vec<Row>,
    session: SharedSession,
) -> Result<BatchSummary> {
    let use_tui = std::io::stderr().is_terminal();
    let mut terminal: Option<DefaultTerminal> = if use_tui {
        Some(crate::tui::terminal::init())
    } else {
        None
    };

    let loop_result = run_loop(&mut plan, runtime, pending_rows, &session, &mut terminal);

    if use_tui {
        crate::tui::terminal::restore();
    }

    // End-of-run finalization (snapshot save) runs even if the loop returned
    // an error so partial successes can still be rolled back. If finish_run
    // itself fails we surface its error only when the loop succeeded — the
    // loop's error is more important and we don't want to mask it.
    let finish_result = session.finish_run();

    match (loop_result, finish_result) {
        (Ok(summary), Ok(())) => Ok(summary),
        (Ok(_), Err(e)) => Err(e),
        (Err(e), Ok(())) => Err(e),
        (Err(loop_err), Err(finish_err)) => {
            eprintln!("Warning: finish_run failed after run error: {finish_err}");
            Err(loop_err)
        }
    }
}

fn run_loop(
    plan: &mut BatchPlan,
    runtime: &ControllerRuntime,
    initial_rows: Vec<Row>,
    session: &SharedSession,
    terminal: &mut Option<DefaultTerminal>,
) -> Result<BatchSummary> {
    let mut current_rows = initial_rows;
    let mut last_summary: Option<BatchSummary> = None;
    let mut is_first_iteration = true;

    loop {
        let batch_config = BatchConfig {
            batch_size: runtime.batch_size,
            retries: runtime.retries,
            model: runtime.model.clone(),
        };

        // Recreate the channel each iteration so the worker can drop the
        // sender on exit and let the renderer's loop terminate cleanly.
        let (event_tx, event_rx) = mpsc::channel::<BatchEvent>();
        let cancel = Arc::new(AtomicBool::new(false));
        let (start_tx, start_rx) = mpsc::sync_channel::<()>(1);

        let session_for_worker = Arc::clone(session);
        let process_fn = session.process_fn();
        let on_row_done = session.on_row_done();
        let id_extractor = session.id_extractor();
        let cancel_for_engine = Arc::clone(&cancel);
        let rows_for_engine = current_rows.clone();
        let plan_run_total = plan.run_total;

        let engine_handle = thread::spawn(move || -> Option<Result<BatchSummary>> {
            // Wait for start signal. Sender dropped means user cancelled at preflight.
            if start_rx.recv().is_err() {
                return None;
            }

            let result = run_batch(
                rows_for_engine,
                process_fn,
                &batch_config,
                on_row_done,
                id_extractor,
                event_tx.clone(),
                cancel_for_engine,
            );

            // Per-iteration finalization (flush, error log, build summary).
            // Errors here become Fatal events to the renderer.
            let summary_result = match session_for_worker.finish_iteration(&result, plan_run_total)
            {
                Ok(summary) => {
                    let _ = event_tx.send(BatchEvent::RunDone(summary.clone()));
                    Ok(summary)
                }
                Err(e) => {
                    let _ = event_tx.send(BatchEvent::Fatal(e.to_string()));
                    Err(e)
                }
            };

            // event_tx dropped here, closing the channel for the renderer
            Some(summary_result)
        });

        let initial_mode = if is_first_iteration {
            AppMode::Preflight
        } else {
            // Skip preflight on retries — controller already signaled engine
            // start (we'll send via start_tx below before run_tui)
            AppMode::Running(RunState::from_plan(plan))
        };

        // For retry iterations the TUI starts in Running mode, so we must
        // signal the engine ourselves before entering the TUI loop.
        let start_tx_for_tui = if is_first_iteration {
            Some(start_tx)
        } else {
            let _ = start_tx.send(());
            None
        };

        let tui_result = if let Some(term) = terminal.as_mut() {
            run_tui(
                term,
                plan,
                initial_mode,
                event_rx,
                Arc::clone(&cancel),
                start_tx_for_tui,
            )?
        } else {
            // Plain mode: signal start immediately and stream events.
            // For retry iterations this also fires (start_tx_for_tui is None
            // because we already sent above).
            if let Some(tx) = start_tx_for_tui {
                let _ = tx.send(());
            }
            run_plain_renderer(event_rx, plan.run_total);
            BatchTuiResult::Done
        };

        let join_result = engine_handle.join().unwrap();

        match (&tui_result, join_result) {
            (BatchTuiResult::Cancelled, None) => {
                // User cancelled at preflight before any run started.
                return Ok(empty_summary(plan));
            }
            (BatchTuiResult::Cancelled, Some(Ok(summary))) => {
                // Force-quit during a running iteration but the engine
                // happened to finalize anyway. Surface the partial summary.
                return Ok(summary);
            }
            (BatchTuiResult::Cancelled, Some(Err(_))) => {
                // Force-quit, finalization failed. Surface what we have.
                return Ok(last_summary.unwrap_or_else(|| empty_summary(plan)));
            }
            (_, Some(Err(e))) => return Err(e),
            (_, Some(Ok(summary))) => {
                last_summary = Some(summary);
            }
            (_, None) => {
                // Engine never ran (cancel at preflight) but TUI returned Done — shouldn't happen
                return Ok(empty_summary(plan));
            }
        }

        match tui_result {
            BatchTuiResult::Done | BatchTuiResult::Cancelled => break,
            BatchTuiResult::RetryFailed(failed_rows) => {
                let next_rows = session.retry_transform(failed_rows);
                if next_rows.is_empty() {
                    break;
                }
                plan.rows = session.row_descriptors(&next_rows);
                plan.run_total = next_rows.len();
                current_rows = next_rows;
                is_first_iteration = false;
                continue;
            }
        }
    }

    Ok(last_summary.unwrap_or_else(|| empty_summary(plan)))
}

fn empty_summary(plan: &BatchPlan) -> BatchSummary {
    BatchSummary {
        planned_total: plan.run_total,
        processed_total: 0,
        succeeded: 0,
        failed: 0,
        interrupted: true,
        input_units: 0,
        output_units: 0,
        cost: 0.0,
        elapsed: std::time::Duration::ZERO,
        model: plan.model.clone(),
        metrics_label: plan.metrics_label,
        show_cost: plan.show_cost,
        headline: "Cancelled".into(),
        completion_fields: Vec::new(),
        failed_rows: Vec::new(),
        can_retry_failed: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::batch::engine::{IdExtractor, OnRowDone, OnRowDoneAction, ProcessFn};
    use crate::batch::events::{BatchEvent, FailedRowInfo, InfoField, RowDescriptor, RowState};
    use crate::batch::report::RowOutcome;
    use crate::batch::session::BatchSession;
    use serde_json::json;
    use std::sync::Mutex;
    use std::time::Duration;

    struct MockSession {
        finish_iteration_calls: Mutex<u32>,
        finish_run_calls: Mutex<u32>,
    }

    impl MockSession {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                finish_iteration_calls: Mutex::new(0),
                finish_run_calls: Mutex::new(0),
            })
        }
    }

    impl BatchSession for MockSession {
        fn process_fn(&self) -> ProcessFn {
            Arc::new(|row| Ok((row.clone(), None)))
        }

        fn id_extractor(&self) -> IdExtractor {
            Arc::new(|row| {
                row.get("noteId")
                    .and_then(|v| v.as_str().map(String::from))
                    .unwrap_or_default()
            })
        }

        fn row_descriptors(&self, rows: &[Row]) -> Vec<RowDescriptor> {
            rows.iter()
                .enumerate()
                .map(|(i, row)| RowDescriptor {
                    index: i,
                    id: row
                        .get("noteId")
                        .and_then(|v| v.as_str().map(String::from))
                        .unwrap_or_default(),
                    preview: String::new(),
                })
                .collect()
        }

        fn finish_iteration(
            &self,
            result: &super::super::engine::EngineRunResult,
            plan_run_total: usize,
        ) -> Result<BatchSummary> {
            *self.finish_iteration_calls.lock().unwrap() += 1;
            let succeeded = result
                .outcomes
                .iter()
                .filter(|o| matches!(o, RowOutcome::Success(_)))
                .count();
            let failed_rows: Vec<FailedRowInfo> = result
                .outcomes
                .iter()
                .filter_map(|o| match o {
                    RowOutcome::Failure { row, error } => Some(FailedRowInfo {
                        id: String::new(),
                        error: error.clone(),
                        row_data: row.clone(),
                    }),
                    _ => None,
                })
                .collect();
            Ok(BatchSummary {
                planned_total: plan_run_total,
                processed_total: result.outcomes.len(),
                succeeded,
                failed: failed_rows.len(),
                interrupted: result.interrupted,
                input_units: result.usage.input,
                output_units: result.usage.output,
                cost: 0.0,
                elapsed: result.elapsed,
                model: Some("test-model".into()),
                metrics_label: "Tokens",
                show_cost: true,
                headline: "Test complete".into(),
                completion_fields: vec![InfoField {
                    label: "Result".into(),
                    value: format!("{} ok", succeeded),
                }],
                failed_rows,
                can_retry_failed: false,
            })
        }

        fn finish_run(&self) -> Result<()> {
            *self.finish_run_calls.lock().unwrap() += 1;
            Ok(())
        }
    }

    fn make_plan(total: usize) -> BatchPlan {
        BatchPlan {
            item_name_singular: "row",
            item_name_plural: "rows",
            rows: (0..total)
                .map(|i| RowDescriptor {
                    index: i,
                    id: i.to_string(),
                    preview: String::new(),
                })
                .collect(),
            run_total: total,
            model: Some("test-model".into()),
            prompt_path: Some("test.txt".into()),
            output_field: Some("TestField".into()),
            batch_size: 2,
            retries: 0,
            sample_prompt: None,
            metrics_label: "Tokens",
            show_cost: true,
            preflight_fields: vec![],
        }
    }

    fn make_runtime() -> ControllerRuntime {
        ControllerRuntime {
            batch_size: 2,
            retries: 0,
            model: Some("test-model".into()),
        }
    }

    fn make_rows(n: usize) -> Vec<Row> {
        (0..n)
            .map(|i| {
                let mut row = Row::new();
                row.insert("noteId".into(), json!(i.to_string()));
                row
            })
            .collect()
    }

    /// In a non-tty environment (cargo test), the controller takes the plain path
    /// and the renderer must terminate after the worker drops the sender.
    #[test]
    fn plain_path_terminates_and_calls_finalization() {
        let session = MockSession::new();
        let plan = make_plan(3);
        let runtime = make_runtime();
        let rows = make_rows(3);

        let summary =
            run_batch_controller(plan, &runtime, rows, session.clone() as SharedSession).unwrap();

        assert_eq!(summary.processed_total, 3);
        assert_eq!(summary.succeeded, 3);
        assert_eq!(summary.failed, 0);
        assert_eq!(*session.finish_iteration_calls.lock().unwrap(), 1);
        assert_eq!(*session.finish_run_calls.lock().unwrap(), 1);
    }

    #[test]
    fn empty_input_still_finalizes() {
        let session = MockSession::new();
        let plan = make_plan(0);
        let runtime = make_runtime();

        let summary =
            run_batch_controller(plan, &runtime, vec![], session.clone() as SharedSession).unwrap();

        assert_eq!(summary.processed_total, 0);
        assert_eq!(*session.finish_iteration_calls.lock().unwrap(), 1);
        assert_eq!(*session.finish_run_calls.lock().unwrap(), 1);
    }

    #[test]
    fn _suppress_unused_warnings() {
        // Touch types to keep them imported when most tests are conditional.
        let _: Option<BatchEvent> = None;
        let _: Option<RowState> = None;
        let _: Option<OnRowDone> = None;
        let _: Option<OnRowDoneAction> = None;
        let _: Duration = Duration::ZERO;
    }
}

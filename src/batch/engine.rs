use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use crate::data::Row;
use crate::data::rows::get_note_id;
use crate::llm::pricing;

use super::error::BatchError;
use super::events::{BatchEvent, BatchSummary, FailedRowInfo, RowState, RowUpdate};
use super::report::{RowOutcome, TokenStats};

/// Configuration for the batch engine.
pub struct BatchConfig {
    pub batch_size: u32,
    pub retries: u32,
    pub model: String,
}

/// Callback invoked after each row completes (success or failure).
/// Returns true to abort the batch (e.g. when an Anki flush fails).
pub type OnRowDone = Box<dyn Fn(&RowOutcome) -> bool + Send + Sync>;

/// The function that processes a single row. Returns the updated row and
/// optional token usage. Errors are retried unless they are `BatchError::Fatal`.
pub type ProcessFn =
    Box<dyn Fn(&Row) -> Result<(Row, Option<(u64, u64)>), BatchError> + Send + Sync>;

/// Run batch processing over a set of rows with bounded concurrency and retries.
///
/// Returns (outcomes, token stats, interrupted). Outcomes are in completion
/// order, not input order. When interrupted, only started rows are present.
///
/// Events are emitted to `event_tx` for UI rendering. All sends use `.ok()`
/// so a dropped receiver (e.g. user quit TUI) won't panic the engine.
pub fn run_batch(
    rows: Vec<Row>,
    process: ProcessFn,
    config: &BatchConfig,
    on_row_done: Option<OnRowDone>,
    event_tx: mpsc::Sender<BatchEvent>,
    cancel: Arc<AtomicBool>,
) -> (Vec<RowOutcome>, TokenStats, bool) {
    let total = rows.len();
    let tokens = Mutex::new(TokenStats::default());
    let completed: Mutex<Vec<RowOutcome>> = Mutex::new(Vec::new());
    let next_index = AtomicUsize::new(0);
    let start = Instant::now();

    std::thread::scope(|s| {
        for _ in 0..config.batch_size {
            s.spawn(|| {
                loop {
                    if cancel.load(Ordering::SeqCst) {
                        break;
                    }

                    let idx = next_index.fetch_add(1, Ordering::SeqCst);
                    if idx >= total {
                        break;
                    }

                    let row = &rows[idx];
                    let ctx = RetryCtx {
                        process: &process,
                        max_retries: config.retries,
                        tokens: &tokens,
                        event_tx: &event_tx,
                        cancel: &cancel,
                        model: &config.model,
                    };
                    let Some(outcome) = process_with_retry(row, idx, &ctx) else {
                        // Row was cancelled — don't record it as a result
                        break;
                    };

                    // Notify callback — abort if it returns true
                    if let Some(ref cb) = on_row_done
                        && cb(&outcome)
                    {
                        cancel.store(true, Ordering::SeqCst);
                    }

                    completed.lock().unwrap().push(outcome);
                }
            });
        }
    });

    let elapsed = start.elapsed();
    let was_interrupted = cancel.load(Ordering::SeqCst);
    let outcomes = completed.into_inner().unwrap();
    let tokens = tokens.into_inner().unwrap();

    let succeeded = outcomes
        .iter()
        .filter(|o| matches!(o, RowOutcome::Success(_)))
        .count();
    let failed = outcomes.len() - succeeded;

    let cost = pricing::calculate_cost(&config.model, tokens.input, tokens.output);
    let failed_rows: Vec<FailedRowInfo> = outcomes
        .iter()
        .filter_map(|o| match o {
            RowOutcome::Failure { row, error } => Some(FailedRowInfo {
                id: get_note_id(row).unwrap_or_default(),
                error: error.clone(),
                row_data: row.clone(),
            }),
            _ => None,
        })
        .collect();

    event_tx
        .send(BatchEvent::RunDone(BatchSummary {
            total: outcomes.len(),
            succeeded,
            failed,
            input_tokens: tokens.input,
            output_tokens: tokens.output,
            cost,
            elapsed,
            interrupted: was_interrupted,
            output_path: String::new(), // filled by caller
            model: config.model.clone(),
            failed_rows,
        }))
        .ok();

    (outcomes, tokens, was_interrupted)
}

/// Shared context for retry processing, bundled to avoid too many arguments.
struct RetryCtx<'a> {
    process: &'a ProcessFn,
    max_retries: u32,
    tokens: &'a Mutex<TokenStats>,
    event_tx: &'a mpsc::Sender<BatchEvent>,
    cancel: &'a AtomicBool,
    model: &'a str,
}

/// Process a single row with retry and exponential backoff.
/// Token usage is accumulated into `tokens` on each successful attempt.
/// Events are emitted for each state transition.
/// Returns `None` if the row was cancelled (not a real failure).
fn process_with_retry(row: &Row, index: usize, ctx: &RetryCtx<'_>) -> Option<RowOutcome> {
    let start = Instant::now();
    let id = get_note_id(row).unwrap_or_default();
    let mut last_error = String::new();

    ctx.event_tx
        .send(BatchEvent::RowStateChanged(RowUpdate {
            index,
            id: id.clone(),
            state: RowState::Running,
            attempt: 1,
            usage: None,
            elapsed: Duration::ZERO,
        }))
        .ok();

    for attempt in 0..=ctx.max_retries {
        if ctx.cancel.load(Ordering::SeqCst) {
            ctx.event_tx
                .send(BatchEvent::RowStateChanged(RowUpdate {
                    index,
                    id: id.clone(),
                    state: RowState::Cancelled,
                    attempt: attempt + 1,
                    usage: None,
                    elapsed: start.elapsed(),
                }))
                .ok();
            return None;
        }

        if attempt > 0 {
            let backoff =
                Duration::from_millis(1000 * 2u64.pow(attempt - 1)).min(Duration::from_secs(30));

            ctx.event_tx
                .send(BatchEvent::RowStateChanged(RowUpdate {
                    index,
                    id: id.clone(),
                    state: RowState::Retrying {
                        error: last_error.clone(),
                    },
                    attempt: attempt + 1,
                    usage: None,
                    elapsed: start.elapsed(),
                }))
                .ok();

            ctx.event_tx
                .send(BatchEvent::Log(format!(
                    "Retry {attempt}/{}: {}",
                    ctx.max_retries, &last_error
                )))
                .ok();

            std::thread::sleep(backoff);

            if ctx.cancel.load(Ordering::SeqCst) {
                ctx.event_tx
                    .send(BatchEvent::RowStateChanged(RowUpdate {
                        index,
                        id: id.clone(),
                        state: RowState::Cancelled,
                        attempt: attempt + 1,
                        usage: None,
                        elapsed: start.elapsed(),
                    }))
                    .ok();
                return None;
            }
        }

        match (ctx.process)(row) {
            Ok((updated_row, usage)) => {
                if let Some((input, output)) = usage {
                    let mut t = ctx.tokens.lock().unwrap();
                    t.add(input, output);
                    let cost = pricing::calculate_cost(ctx.model, t.input, t.output);
                    ctx.event_tx
                        .send(BatchEvent::CostUpdate {
                            input_tokens: t.input,
                            output_tokens: t.output,
                            cost,
                        })
                        .ok();
                }
                ctx.event_tx
                    .send(BatchEvent::RowStateChanged(RowUpdate {
                        index,
                        id: id.clone(),
                        state: RowState::Succeeded,
                        attempt: attempt + 1,
                        usage,
                        elapsed: start.elapsed(),
                    }))
                    .ok();
                return Some(RowOutcome::Success(updated_row));
            }
            Err(e) => {
                last_error = e.to_string();
                if !e.is_retryable() {
                    break;
                }
            }
        }
    }

    ctx.event_tx
        .send(BatchEvent::RowStateChanged(RowUpdate {
            index,
            id: id.clone(),
            state: RowState::Failed {
                error: last_error.clone(),
            },
            attempt: ctx.max_retries + 1,
            usage: None,
            elapsed: start.elapsed(),
        }))
        .ok();

    Some(make_failure(row, last_error))
}

fn make_failure(row: &Row, error: String) -> RowOutcome {
    let mut failed_row = row.clone();
    failed_row.insert(
        super::report::ERROR_FIELD.to_string(),
        serde_json::Value::String(error.clone()),
    );
    RowOutcome::Failure {
        row: failed_row,
        error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::AtomicU32;

    fn make_row(id: i64) -> Row {
        let mut row = Row::new();
        row.insert("noteId".into(), json!(id));
        row.insert("Front".into(), json!(format!("row-{id}")));
        row
    }

    fn config(retries: u32) -> BatchConfig {
        BatchConfig {
            batch_size: 1,
            retries,
            model: "test-model".into(),
        }
    }

    fn test_run_batch(
        rows: Vec<Row>,
        process: ProcessFn,
        config: &BatchConfig,
        on_row_done: Option<OnRowDone>,
    ) -> (Vec<RowOutcome>, TokenStats, bool) {
        let (tx, _rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        run_batch(rows, process, config, on_row_done, tx, cancel)
    }

    #[test]
    fn all_rows_succeed() {
        let rows = vec![make_row(1), make_row(2), make_row(3)];
        let process: ProcessFn = Box::new(|row| {
            let mut out = row.clone();
            out.insert("Back".into(), json!("done"));
            Ok((out, Some((10, 5))))
        });

        let (outcomes, tokens, interrupted) = test_run_batch(rows, process, &config(0), None);

        assert!(!interrupted);
        assert_eq!(outcomes.len(), 3);
        assert!(outcomes.iter().all(|o| matches!(o, RowOutcome::Success(_))));
        assert_eq!(tokens.input, 30);
        assert_eq!(tokens.output, 15);
    }

    #[test]
    fn all_rows_fail() {
        let rows = vec![make_row(1)];
        let process: ProcessFn = Box::new(|_| Err(BatchError::Processing("always fails".into())));

        let (outcomes, _, _) = test_run_batch(rows, process, &config(0), None);

        assert_eq!(outcomes.len(), 1);
        assert!(
            matches!(&outcomes[0], RowOutcome::Failure { error, .. } if error == "always fails")
        );
    }

    #[test]
    fn retry_succeeds_on_second_attempt() {
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = Arc::clone(&attempts);

        let rows = vec![make_row(1)];
        let process: ProcessFn = Box::new(move |row| {
            let n = attempts_clone.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                Err(BatchError::Processing("transient".into()))
            } else {
                Ok((row.clone(), Some((10, 5))))
            }
        });

        let (outcomes, tokens, _) = test_run_batch(rows, process, &config(1), None);

        assert_eq!(outcomes.len(), 1);
        assert!(matches!(&outcomes[0], RowOutcome::Success(_)));
        assert_eq!(tokens.input, 10);
    }

    #[test]
    fn fatal_error_skips_retries() {
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = Arc::clone(&attempts);

        let rows = vec![make_row(1)];
        let process: ProcessFn = Box::new(move |_| {
            attempts_clone.fetch_add(1, Ordering::SeqCst);
            Err(BatchError::Fatal("bad template".into()))
        });

        let (outcomes, _, _) = test_run_batch(rows, process, &config(3), None);

        assert_eq!(outcomes.len(), 1);
        assert!(matches!(&outcomes[0], RowOutcome::Failure { .. }));
        // Fatal should not retry — only 1 attempt
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn failure_row_has_error_field() {
        let rows = vec![make_row(1)];
        let process: ProcessFn = Box::new(|_| Err(BatchError::Processing("oops".into())));

        let (outcomes, _, _) = test_run_batch(rows, process, &config(0), None);

        if let RowOutcome::Failure { row, .. } = &outcomes[0] {
            assert_eq!(row["_error"], json!("oops"));
        } else {
            panic!("expected failure");
        }
    }

    #[test]
    fn on_row_done_callback_is_called() {
        let count = Arc::new(AtomicU32::new(0));
        let count_clone = Arc::clone(&count);

        let rows = vec![make_row(1), make_row(2)];
        let process: ProcessFn = Box::new(|row| Ok((row.clone(), None)));
        let on_done: OnRowDone = Box::new(move |_| {
            count_clone.fetch_add(1, Ordering::SeqCst);
            false
        });

        test_run_batch(rows, process, &config(0), Some(on_done));

        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn no_usage_means_zero_tokens() {
        let rows = vec![make_row(1)];
        let process: ProcessFn = Box::new(|row| Ok((row.clone(), None)));

        let (_, tokens, _) = test_run_batch(rows, process, &config(0), None);

        assert_eq!(tokens.total(), 0);
    }

    #[test]
    fn cancel_stops_processing() {
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_clone = Arc::clone(&cancel);
        let processed = Arc::new(AtomicU32::new(0));
        let processed_clone = Arc::clone(&processed);

        let rows = vec![make_row(1), make_row(2), make_row(3)];
        let process: ProcessFn = Box::new(move |row| {
            let n = processed_clone.fetch_add(1, Ordering::SeqCst);
            if n >= 1 {
                cancel_clone.store(true, Ordering::SeqCst);
            }
            Ok((row.clone(), None))
        });

        let (tx, _rx) = mpsc::channel();
        let (outcomes, _, interrupted) = run_batch(rows, process, &config(0), None, tx, cancel);

        assert!(interrupted);
        // With batch_size=1, at most 2 rows processed before cancel takes effect
        assert!(outcomes.len() <= 2);
    }
}

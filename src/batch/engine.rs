use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use crate::data::Row;
use crate::llm::pricing;

use super::error::BatchError;
use super::events::{BatchEvent, RowState, RowUpdate};
use super::report::{RowOutcome, UsageStats};

/// Configuration for the batch engine.
pub struct BatchConfig {
    pub batch_size: u32,
    pub retries: u32,
    /// Model name used for pricing lookups. `None` means no cost tracking
    /// (e.g. TTS sessions).
    pub model: Option<String>,
}

/// Action returned by the per-row callback. `Abort` stops the engine and
/// surfaces the message to the controller via `EngineRunResult.abort_reason`.
pub enum OnRowDoneAction {
    Continue,
    Abort(String),
}

/// Callback invoked after each row completes (success or failure).
pub type OnRowDone = Arc<dyn Fn(&RowOutcome) -> OnRowDoneAction + Send + Sync>;

/// The function that processes a single row. Returns the updated row and
/// optional token usage. Errors are retried unless they are `BatchError::Fatal`.
pub type ProcessFn =
    Arc<dyn Fn(&Row) -> Result<(Row, Option<(u64, u64)>), BatchError> + Send + Sync>;

/// Extracts a stable display ID from a row. Different commands store IDs
/// under different keys (file mode uses `noteId`/`id`/`Id`; deck mode uses
/// the private `__note_id` field).
pub type IdExtractor = Arc<dyn Fn(&Row) -> String + Send + Sync>;

/// Pure result returned from `run_batch`. Contains all data needed by the
/// controller to drive sink finalization and build a `BatchSummary`. The
/// planned total isn't included here — the controller already knows it from
/// the `BatchPlan`, so duplicating it would invite drift.
pub struct EngineRunResult {
    pub outcomes: Vec<RowOutcome>,
    pub usage: UsageStats,
    pub elapsed: Duration,
    pub interrupted: bool,
    pub abort_reason: Option<String>,
}

/// Run batch processing over a set of rows with bounded concurrency and retries.
///
/// Emits `RowStateChanged`, `Log`, and `CostUpdate` events to `event_tx` for
/// UI rendering. **Does not** emit `RunDone` or `Fatal` — the controller is
/// responsible for those after sink finalization. All sends use `.ok()` so a
/// dropped receiver won't panic the engine.
pub fn run_batch(
    rows: Vec<Row>,
    process: ProcessFn,
    config: &BatchConfig,
    on_row_done: Option<OnRowDone>,
    id_extractor: IdExtractor,
    event_tx: mpsc::Sender<BatchEvent>,
    cancel: Arc<AtomicBool>,
) -> EngineRunResult {
    let total = rows.len();
    let usage = Mutex::new(UsageStats::default());
    let completed: Mutex<Vec<RowOutcome>> = Mutex::new(Vec::new());
    let abort_reason: Mutex<Option<String>> = Mutex::new(None);
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
                        usage: &usage,
                        event_tx: &event_tx,
                        cancel: &cancel,
                        model: config.model.as_deref(),
                        id_extractor: &id_extractor,
                    };
                    let Some(outcome) = process_with_retry(row, idx, &ctx) else {
                        // Row was cancelled — don't record it as a result
                        break;
                    };

                    // Notify callback — abort if it returns Abort
                    if let Some(ref cb) = on_row_done
                        && let OnRowDoneAction::Abort(msg) = cb(&outcome)
                    {
                        *abort_reason.lock().unwrap() = Some(msg);
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
    let usage = usage.into_inner().unwrap();
    let abort_reason = abort_reason.into_inner().unwrap();

    EngineRunResult {
        outcomes,
        usage,
        elapsed,
        interrupted: was_interrupted,
        abort_reason,
    }
}

/// Shared context for retry processing, bundled to avoid too many arguments.
struct RetryCtx<'a> {
    process: &'a ProcessFn,
    max_retries: u32,
    usage: &'a Mutex<UsageStats>,
    event_tx: &'a mpsc::Sender<BatchEvent>,
    cancel: &'a AtomicBool,
    /// LLM model name used for cost lookups. `None` skips cost computation.
    model: Option<&'a str>,
    id_extractor: &'a IdExtractor,
}

/// Process a single row with retry and exponential backoff.
/// Token usage is accumulated into `tokens` on each successful attempt.
/// Events are emitted for each state transition.
/// Returns `None` if the row was cancelled (not a real failure).
fn process_with_retry(row: &Row, index: usize, ctx: &RetryCtx<'_>) -> Option<RowOutcome> {
    let start = Instant::now();
    let id = (ctx.id_extractor)(row);
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
                    let mut t = ctx.usage.lock().unwrap();
                    t.add(input, output);
                    let cost = match ctx.model {
                        Some(m) => pricing::calculate_cost(m, t.input, t.output),
                        None => 0.0,
                    };
                    ctx.event_tx
                        .send(BatchEvent::CostUpdate {
                            input_units: t.input,
                            output_units: t.output,
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
    use crate::data::rows::get_note_id;
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
            model: Some("test-model".into()),
        }
    }

    fn default_id_extractor() -> IdExtractor {
        Arc::new(|row| get_note_id(row).unwrap_or_default())
    }

    fn test_run_batch(
        rows: Vec<Row>,
        process: ProcessFn,
        config: &BatchConfig,
        on_row_done: Option<OnRowDone>,
    ) -> EngineRunResult {
        let (tx, _rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        run_batch(
            rows,
            process,
            config,
            on_row_done,
            default_id_extractor(),
            tx,
            cancel,
        )
    }

    #[test]
    fn all_rows_succeed() {
        let rows = vec![make_row(1), make_row(2), make_row(3)];
        let process: ProcessFn = Arc::new(|row| {
            let mut out = row.clone();
            out.insert("Back".into(), json!("done"));
            Ok((out, Some((10, 5))))
        });

        let result = test_run_batch(rows, process, &config(0), None);

        assert!(!result.interrupted);
        assert_eq!(result.outcomes.len(), 3);
        assert!(
            result
                .outcomes
                .iter()
                .all(|o| matches!(o, RowOutcome::Success(_)))
        );
        assert_eq!(result.usage.input, 30);
        assert_eq!(result.usage.output, 15);
    }

    #[test]
    fn all_rows_fail() {
        let rows = vec![make_row(1)];
        let process: ProcessFn = Arc::new(|_| Err(BatchError::Processing("always fails".into())));

        let result = test_run_batch(rows, process, &config(0), None);

        assert_eq!(result.outcomes.len(), 1);
        assert!(
            matches!(&result.outcomes[0], RowOutcome::Failure { error, .. } if error == "always fails")
        );
    }

    #[test]
    fn retry_succeeds_on_second_attempt() {
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = Arc::clone(&attempts);

        let rows = vec![make_row(1)];
        let process: ProcessFn = Arc::new(move |row| {
            let n = attempts_clone.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                Err(BatchError::Processing("transient".into()))
            } else {
                Ok((row.clone(), Some((10, 5))))
            }
        });

        let result = test_run_batch(rows, process, &config(1), None);

        assert_eq!(result.outcomes.len(), 1);
        assert!(matches!(&result.outcomes[0], RowOutcome::Success(_)));
        assert_eq!(result.usage.input, 10);
    }

    #[test]
    fn fatal_error_skips_retries() {
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = Arc::clone(&attempts);

        let rows = vec![make_row(1)];
        let process: ProcessFn = Arc::new(move |_| {
            attempts_clone.fetch_add(1, Ordering::SeqCst);
            Err(BatchError::Fatal("bad template".into()))
        });

        let result = test_run_batch(rows, process, &config(3), None);

        assert_eq!(result.outcomes.len(), 1);
        assert!(matches!(&result.outcomes[0], RowOutcome::Failure { .. }));
        // Fatal should not retry — only 1 attempt
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn failure_row_has_error_field() {
        let rows = vec![make_row(1)];
        let process: ProcessFn = Arc::new(|_| Err(BatchError::Processing("oops".into())));

        let result = test_run_batch(rows, process, &config(0), None);

        if let RowOutcome::Failure { row, .. } = &result.outcomes[0] {
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
        let process: ProcessFn = Arc::new(|row| Ok((row.clone(), None)));
        let on_done: OnRowDone = Arc::new(move |_| {
            count_clone.fetch_add(1, Ordering::SeqCst);
            OnRowDoneAction::Continue
        });

        test_run_batch(rows, process, &config(0), Some(on_done));

        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn no_usage_means_zero_tokens() {
        let rows = vec![make_row(1)];
        let process: ProcessFn = Arc::new(|row| Ok((row.clone(), None)));

        let result = test_run_batch(rows, process, &config(0), None);

        assert_eq!(result.usage.total(), 0);
    }

    #[test]
    fn cancel_stops_processing() {
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_clone = Arc::clone(&cancel);
        let processed = Arc::new(AtomicU32::new(0));
        let processed_clone = Arc::clone(&processed);

        let rows = vec![make_row(1), make_row(2), make_row(3)];
        let process: ProcessFn = Arc::new(move |row| {
            let n = processed_clone.fetch_add(1, Ordering::SeqCst);
            if n >= 1 {
                cancel_clone.store(true, Ordering::SeqCst);
            }
            Ok((row.clone(), None))
        });

        let (tx, _rx) = mpsc::channel();
        let result = run_batch(
            rows,
            process,
            &config(0),
            None,
            default_id_extractor(),
            tx,
            cancel,
        );

        assert!(result.interrupted);
        // With batch_size=1, at most 2 rows processed before cancel takes effect
        assert!(result.outcomes.len() <= 2);
    }

    #[test]
    fn abort_callback_stops_processing() {
        let rows = vec![make_row(1), make_row(2), make_row(3)];
        let process: ProcessFn = Arc::new(|row| Ok((row.clone(), None)));
        let on_done: OnRowDone = Arc::new(|_| OnRowDoneAction::Abort("sink failed".into()));

        let result = test_run_batch(rows, process, &config(0), Some(on_done));
        assert!(result.interrupted);
        assert_eq!(result.abort_reason.as_deref(), Some("sink failed"));
        assert!(!result.outcomes.is_empty());
    }
}

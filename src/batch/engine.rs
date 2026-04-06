use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use indicatif::{ProgressBar, ProgressStyle};

use crate::data::Row;
use crate::llm::pricing;

use super::error::BatchError;
use super::report::{RowOutcome, TokenStats};

/// Configuration for the batch engine.
pub struct BatchConfig {
    pub batch_size: u32,
    pub retries: u32,
    pub model: String,
}

/// Callback invoked after each row completes (success or failure).
/// Used by file_mode to do incremental writes.
pub type OnRowDone = Box<dyn Fn(&RowOutcome) + Send + Sync>;

/// The function that processes a single row. Returns the updated row and
/// optional token usage. Errors are retried unless they are `BatchError::Fatal`.
pub type ProcessFn =
    Box<dyn Fn(&Row) -> Result<(Row, Option<(u64, u64)>), BatchError> + Send + Sync>;

/// Run batch processing over a set of rows with bounded concurrency and retries.
///
/// Returns (completed outcomes, token stats, whether interrupted).
/// Only rows that were actually started are included in outcomes.
pub fn run_batch(
    rows: Vec<Row>,
    process: ProcessFn,
    config: &BatchConfig,
    on_row_done: Option<OnRowDone>,
) -> (Vec<RowOutcome>, TokenStats, bool) {
    let total = rows.len();
    let interrupted = Arc::new(AtomicBool::new(false));

    // Set up progress bar
    let pb = ProgressBar::new(total as u64);
    pb.set_style(
        ProgressStyle::with_template("Processing |{bar:40.cyan/blue}| {pos}/{len} | Cost: {msg}")
            .unwrap()
            .progress_chars("##-"),
    );

    // Shared state
    let tokens = Arc::new(Mutex::new(TokenStats::default()));
    let completed: Arc<Mutex<Vec<RowOutcome>>> = Arc::new(Mutex::new(Vec::new()));
    let next_index = Arc::new(AtomicUsize::new(0));
    let on_row_done = on_row_done.map(Arc::new);

    // Install SIGINT handler
    let interrupted_clone = interrupted.clone();
    let _ = ctrlc::set_handler(move || {
        interrupted_clone.store(true, Ordering::SeqCst);
    });

    let start = Instant::now();

    std::thread::scope(|s| {
        for _ in 0..config.batch_size {
            let rows = &rows;
            let process = &process;
            let tokens = Arc::clone(&tokens);
            let completed = Arc::clone(&completed);
            let next_index = Arc::clone(&next_index);
            let pb = &pb;
            let config = &config;
            let on_row_done = on_row_done.clone();
            let interrupted = Arc::clone(&interrupted);

            s.spawn(move || {
                loop {
                    if interrupted.load(Ordering::SeqCst) {
                        break;
                    }

                    let idx = next_index.fetch_add(1, Ordering::SeqCst);
                    if idx >= total {
                        break;
                    }

                    let row = &rows[idx];
                    let outcome = process_with_retry(row, process, config.retries, &tokens);

                    // Notify callback
                    if let Some(ref cb) = on_row_done {
                        cb(&outcome);
                    }

                    // Update progress
                    {
                        let t = tokens.lock().unwrap();
                        let cost = pricing::calculate_cost(&config.model, t.input, t.output);
                        pb.set_message(pricing::format_cost(cost));
                    }
                    pb.inc(1);

                    completed.lock().unwrap().push(outcome);
                }
            });
        }
    });

    pb.finish_and_clear();

    let elapsed = start.elapsed();
    let was_interrupted = interrupted.load(Ordering::SeqCst);
    let outcomes = Arc::try_unwrap(completed).unwrap().into_inner().unwrap();
    let tokens = Arc::try_unwrap(tokens).unwrap().into_inner().unwrap();

    let succeeded = outcomes
        .iter()
        .filter(|o| matches!(o, RowOutcome::Success(_)))
        .count();
    let failed = outcomes.len() - succeeded;
    super::report::print_summary(&config.model, &tokens, succeeded, failed, elapsed);

    if was_interrupted {
        eprintln!("\nInterrupted by user. Partial results saved.");
    }

    (outcomes, tokens, was_interrupted)
}

/// Process a single row with retry and exponential backoff.
/// Token usage is accumulated into `tokens` on each successful attempt.
fn process_with_retry(
    row: &Row,
    process: &ProcessFn,
    max_retries: u32,
    tokens: &Arc<Mutex<TokenStats>>,
) -> RowOutcome {
    let mut last_error = String::new();

    for attempt in 0..=max_retries {
        if attempt > 0 {
            let backoff = Duration::from_millis(1000 * 2u64.pow(attempt - 1));
            let backoff = backoff.min(Duration::from_secs(30));
            eprintln!("  Retry {attempt}/{max_retries}: {last_error}",);
            std::thread::sleep(backoff);
        }

        match process(row) {
            Ok((updated_row, usage)) => {
                if let Some((input, output)) = usage {
                    tokens.lock().unwrap().add(input, output);
                }
                return RowOutcome::Success(updated_row);
            }
            Err(e) => {
                last_error = e.to_string();
                if !e.is_retryable() {
                    break;
                }
            }
        }
    }

    // All retries exhausted — return failure with _error field
    let mut failed_row = row.clone();
    failed_row.insert(
        "_error".to_string(),
        serde_json::Value::String(last_error.clone()),
    );
    RowOutcome::Failure {
        row: failed_row,
        error: last_error,
    }
}

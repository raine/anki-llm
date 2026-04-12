use std::sync::Arc;

use anyhow::Result;

use crate::data::Row;

use super::engine::{EngineRunResult, IdExtractor, OnRowDone, ProcessFn};
use super::events::{BatchSummary, RowDescriptor};
use super::report::ERROR_FIELD;

/// A persistent session backing one logical batch run (which may span multiple
/// retry iterations). Owns the sink (FileWriter / DeckWriter) and any state
/// that must survive across iterations. Implementations are typically wrapped
/// in `Arc` and shared between the controller and the engine worker thread.
pub trait BatchSession: Send + Sync {
    /// The row-processing closure (cheap clone — backed by `Arc`).
    fn process_fn(&self) -> ProcessFn;

    /// Optional callback invoked after each row outcome. Default: None.
    fn on_row_done(&self) -> Option<OnRowDone> {
        None
    }

    /// Extracts a stable display ID from a row.
    fn id_extractor(&self) -> IdExtractor;

    /// Builds row descriptors for a `BatchPlan` (used on retry rebuild).
    fn row_descriptors(&self, rows: &[Row]) -> Vec<RowDescriptor>;

    /// Per-iteration finalization. Called after the engine completes each
    /// iteration (initial run + each retry). Should flush any buffered writes,
    /// rewrite the failure log, and return a fully-populated `BatchSummary`.
    fn finish_iteration(
        &self,
        result: &EngineRunResult,
        plan_run_total: usize,
    ) -> Result<BatchSummary>;

    /// End-of-run finalization. Called once after the user dismisses the TUI
    /// (or plain mode finishes). Use this for snapshot writes that should
    /// only happen once across all retries. Default: no-op.
    fn finish_run(&self) -> Result<()> {
        Ok(())
    }

    /// Transform the failed rows into the next iteration's input rows.
    /// Default: strip the `_error` field and pass through.
    fn retry_transform(&self, failed: Vec<Row>) -> Vec<Row> {
        failed
            .into_iter()
            .map(|mut r| {
                r.shift_remove(ERROR_FIELD);
                r
            })
            .collect()
    }
}

pub type SharedSession = Arc<dyn BatchSession>;

use std::time::Duration;

use crate::data::Row;

/// Engine -> UI events emitted during batch processing.
///
/// `RowStateChanged`, `Log`, `CostUpdate` are emitted by the engine. `RunDone`
/// and `Fatal` are emitted by the controller after sink finalization, never
/// from inside the engine itself.
pub enum BatchEvent {
    RowStateChanged(RowUpdate),
    Log(String),
    CostUpdate {
        input_tokens: u64,
        output_tokens: u64,
        cost: f64,
    },
    RunDone(BatchSummary),
    Fatal(String),
}

/// A label/value pair shown in the preflight or completion banner.
#[derive(Clone)]
pub struct InfoField {
    pub label: String,
    pub value: String,
}

/// Plan data built by the controller before starting the engine.
/// Not an engine event — passed directly to the renderer.
#[derive(Clone)]
pub struct BatchPlan {
    /// Singular noun for items being processed (e.g. "row", "note").
    pub item_name_singular: &'static str,
    /// Plural form of the same noun.
    pub item_name_plural: &'static str,
    pub rows: Vec<RowDescriptor>,
    pub run_total: usize,
    pub model: String,
    pub prompt_path: String,
    pub output_mode: OutputMode,
    pub batch_size: u32,
    pub retries: u32,
    pub sample_prompt: Option<String>,
    /// Caller-supplied label/value pairs shown in the preflight screen
    /// (e.g. "Input", "Output", "Source", "Destination").
    pub preflight_fields: Vec<InfoField>,
}

#[derive(Clone)]
pub enum OutputMode {
    SingleField(String),
    JsonMerge,
}

#[derive(Clone)]
pub struct RowDescriptor {
    pub index: usize,
    pub id: String,
    pub preview: String,
}

pub struct RowUpdate {
    pub index: usize,
    pub id: String,
    pub state: RowState,
    pub attempt: u32,
    pub usage: Option<(u64, u64)>,
    pub elapsed: Duration,
}

#[derive(Clone, Debug)]
pub enum RowState {
    /// Initial state: row not yet picked up by a worker.
    Pending,
    Running,
    Retrying {
        error: String,
    },
    Succeeded,
    Failed {
        error: String,
    },
    Cancelled,
}

#[derive(Clone)]
pub struct FailedRowInfo {
    pub id: String,
    pub error: String,
    pub row_data: Row,
}

#[derive(Clone)]
pub struct BatchSummary {
    /// Number of rows the iteration was supposed to process.
    pub planned_total: usize,
    /// Number of rows that actually completed (success + failure).
    pub processed_total: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub interrupted: bool,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost: f64,
    pub elapsed: Duration,
    pub model: String,
    /// Short headline shown in the success banner (e.g. "Batch complete",
    /// "Updated 42 notes in Anki").
    pub headline: String,
    /// Caller-supplied completion fields shown under the headline.
    pub completion_fields: Vec<InfoField>,
    pub failed_rows: Vec<FailedRowInfo>,
    /// True iff retrying failed rows is meaningful (i.e. the run was not
    /// cancelled or aborted, and there are some failed rows).
    pub can_retry_failed: bool,
}

use std::time::Duration;

use crate::data::Row;

/// Engine -> UI events emitted during batch processing.
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

/// Plan data built by the controller before starting the engine.
/// Not an engine event — passed directly to the renderer.
pub struct BatchPlan {
    pub rows: Vec<RowDescriptor>,
    pub input_total: usize,
    pub resume_skipped: usize,
    pub run_total: usize,
    pub model: String,
    pub prompt_path: String,
    pub output_path: String,
    pub output_mode: OutputMode,
    pub batch_size: u32,
    pub retries: u32,
    pub sample_prompt: Option<String>,
}

pub enum OutputMode {
    SingleField(String),
    JsonMerge,
}

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
    Running,
    Retrying { error: String },
    Succeeded,
    Failed { error: String },
    Cancelled,
}

pub struct FailedRowInfo {
    pub id: String,
    pub error: String,
    pub row_data: Row,
}

pub struct BatchSummary {
    pub total: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost: f64,
    pub elapsed: Duration,
    pub interrupted: bool,
    pub output_path: String,
    pub model: String,
    pub failed_rows: Vec<FailedRowInfo>,
}

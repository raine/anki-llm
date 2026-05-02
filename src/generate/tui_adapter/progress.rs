use std::sync::mpsc;

use super::super::pipeline::{PipelineProgress, PipelineStep};
use super::super::tui::{BackendEvent, StepStatus};

pub(super) struct TuiProgress {
    pub tx: mpsc::Sender<BackendEvent>,
}

impl PipelineProgress for TuiProgress {
    fn log(&self, msg: &str) {
        self.tx.send(BackendEvent::Log(msg.to_string())).ok();
    }

    fn step_start(&self, step: PipelineStep, _detail: Option<&str>) {
        self.tx
            .send(BackendEvent::StepUpdate {
                step,
                status: StepStatus::Running(None),
            })
            .ok();
    }

    fn step_done(&self, step: PipelineStep, detail: Option<String>) {
        self.tx
            .send(BackendEvent::StepUpdate {
                step,
                status: StepStatus::Done(detail),
            })
            .ok();
    }

    fn step_skip(&self, step: PipelineStep) {
        self.tx
            .send(BackendEvent::StepUpdate {
                step,
                status: StepStatus::Skipped,
            })
            .ok();
    }

    fn step_error(&self, step: PipelineStep, detail: &str) {
        self.tx
            .send(BackendEvent::StepUpdate {
                step,
                status: StepStatus::Error(detail.to_string()),
            })
            .ok();
    }

    fn cost_update(&self, input_tokens: u64, output_tokens: u64, cost: f64) {
        self.tx
            .send(BackendEvent::CostUpdate {
                input_tokens,
                output_tokens,
                cost,
            })
            .ok();
    }

    fn thinking_reset(&self) {
        self.tx.send(BackendEvent::ThinkingReset).ok();
    }

    fn thinking_delta(&self, delta: &str) {
        self.tx
            .send(BackendEvent::ThinkingDelta(delta.to_string()))
            .ok();
    }

    fn thinking_clear(&self) {
        self.tx.send(BackendEvent::ThinkingClear).ok();
    }
}

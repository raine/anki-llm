use std::time::{Duration, Instant};

use crate::data::Row;

use super::super::events::{BatchPlan, BatchSummary, RowState, RowUpdate};

pub enum AppMode {
    Preflight,
    Running(RunState),
    Done(DoneState),
    Error(String),
}

pub struct RunState {
    pub rows: Vec<RowStatus>,
    /// Display order: running rows first, then completed, stable within groups.
    pub row_order: Vec<usize>,
    pub scroll: usize,
    pub log: Vec<String>,
    pub log_scroll: u16,
    pub stats: RunStats,
    pub cancelling: bool,
    pub tick: u64,
}

impl RunState {
    pub fn from_plan(plan: &BatchPlan) -> Self {
        let rows: Vec<RowStatus> = plan
            .rows
            .iter()
            .map(|rd| RowStatus {
                id: rd.id.clone(),
                preview: rd.preview.clone(),
                state: RowState::Pending,
                attempt: 0,
                max_attempts: plan.retries + 1,
                elapsed: Duration::ZERO,
                started_at: None,
            })
            .collect();
        let row_order: Vec<usize> = (0..rows.len()).collect();
        RunState {
            rows,
            row_order,
            scroll: 0,
            log: Vec::new(),
            log_scroll: 0,
            stats: RunStats::new(plan.run_total),
            cancelling: false,
            tick: 0,
        }
    }

    pub fn apply_row_update(&mut self, update: RowUpdate) {
        if update.index < self.rows.len() {
            let row = &mut self.rows[update.index];
            if matches!(update.state, RowState::Running) && row.started_at.is_none() {
                row.started_at = Some(Instant::now());
            }
            row.state = update.state.clone();
            row.attempt = update.attempt;
            row.elapsed = update.elapsed;
        }

        // Update stats
        match &update.state {
            RowState::Pending => {
                // Initial placeholder — engine never emits this.
            }
            RowState::Running => {
                self.stats.running += 1;
                self.stats.queued = self.stats.queued.saturating_sub(1);
            }
            RowState::Succeeded => {
                self.stats.running = self.stats.running.saturating_sub(1);
                self.stats.succeeded += 1;
                self.stats.row_durations.push(update.elapsed);
            }
            RowState::Failed { .. } => {
                self.stats.running = self.stats.running.saturating_sub(1);
                self.stats.failed += 1;
                self.stats.row_durations.push(update.elapsed);
            }
            RowState::Cancelled => {
                self.stats.running = self.stats.running.saturating_sub(1);
            }
            RowState::Retrying { .. } => {
                // Still counted as running
            }
        }

        self.rebuild_row_order();
    }

    fn rebuild_row_order(&mut self) {
        // Stable input order — running rows are visually distinct via spinner icon
        // so no need to reorder them to the top.
    }

    pub fn scroll_down(&mut self) {
        if self.scroll + 1 < self.rows.len() {
            self.scroll += 1;
        }
    }

    pub fn scroll_up(&mut self) {
        self.scroll = self.scroll.saturating_sub(1);
    }
}

pub struct RunStats {
    pub total: usize,
    pub queued: usize,
    pub running: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost: f64,
    pub start_time: Instant,
    pub row_durations: Vec<Duration>,
    /// Set when the run completes to freeze the elapsed display.
    pub frozen_elapsed: Option<Duration>,
}

impl RunStats {
    fn new(total: usize) -> Self {
        RunStats {
            total,
            queued: total,
            running: 0,
            succeeded: 0,
            failed: 0,
            input_tokens: 0,
            output_tokens: 0,
            cost: 0.0,
            start_time: Instant::now(),
            row_durations: Vec::new(),
            frozen_elapsed: None,
        }
    }

    pub fn elapsed(&self) -> Duration {
        self.frozen_elapsed
            .unwrap_or_else(|| self.start_time.elapsed())
    }

    pub fn eta(&self) -> Option<Duration> {
        if self.row_durations.is_empty() {
            return None;
        }
        let avg: Duration =
            self.row_durations.iter().sum::<Duration>() / self.row_durations.len() as u32;
        let remaining = self.total - self.succeeded - self.failed;
        Some(avg * remaining as u32)
    }
}

pub struct RowStatus {
    pub id: String,
    pub preview: String,
    pub state: RowState,
    pub attempt: u32,
    pub max_attempts: u32,
    pub elapsed: Duration,
    /// When the row started running, for live elapsed display.
    pub started_at: Option<Instant>,
}

pub struct DoneState {
    pub summary: BatchSummary,
    /// Frozen snapshot of the running screen for continued display.
    pub run: RunState,
    /// Selected failed row index for triage browsing.
    pub cursor: usize,
}

/// Result of a TUI session.
pub enum TuiResult {
    Done,
    RetryFailed(Vec<Row>),
    Cancelled,
}

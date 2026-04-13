/// Field key used to record processing errors on failed rows.
pub const ERROR_FIELD: &str = "_error";

/// Accumulated per-row usage counts across all processed rows.
///
/// The unit is processor-specific: LLM sessions count tokens, TTS sessions
/// count characters. The engine does not interpret the numbers — it just
/// accumulates them and hands them to the renderer, which labels the section
/// from `BatchPlan::metrics_label`.
#[derive(Debug, Default, Clone)]
pub struct UsageStats {
    pub input: u64,
    pub output: u64,
}

impl UsageStats {
    #[cfg(test)]
    pub fn total(&self) -> u64 {
        self.input + self.output
    }

    pub fn add(&mut self, input: u64, output: u64) {
        self.input += input;
        self.output += output;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_stats_add_and_total() {
        let mut stats = UsageStats::default();
        assert_eq!(stats.total(), 0);
        stats.add(100, 50);
        assert_eq!(stats.input, 100);
        assert_eq!(stats.output, 50);
        assert_eq!(stats.total(), 150);
        stats.add(200, 100);
        assert_eq!(stats.total(), 450);
    }
}

/// Result of processing a single row.
#[derive(Debug)]
pub enum RowOutcome {
    /// Row processed successfully. Contains the updated row.
    Success(crate::data::Row),
    /// Row failed after all retries. Contains original row + error message.
    Failure {
        row: crate::data::Row,
        error: String,
    },
}

use crate::llm::pricing;

/// Accumulated token counts across all processed rows.
#[derive(Debug, Default, Clone)]
pub struct TokenStats {
    pub input: u64,
    pub output: u64,
}

impl TokenStats {
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
    fn token_stats_add_and_total() {
        let mut stats = TokenStats::default();
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
        #[allow(dead_code)]
        error: String,
    },
}

/// Print the end-of-run summary to stderr.
pub fn print_summary(
    model: &str,
    tokens: &TokenStats,
    succeeded: usize,
    failed: usize,
    elapsed: std::time::Duration,
) {
    let total = succeeded + failed;
    let cost = pricing::calculate_cost(model, tokens.input, tokens.output);

    eprintln!("\n{}", "=".repeat(60));
    eprintln!("Processing complete");
    eprintln!("{}", "=".repeat(60));
    eprintln!("\nResults:");
    eprintln!("  Processed: {total}");
    eprintln!("  Succeeded: {succeeded}");
    if failed > 0 {
        eprintln!("  Failed:    {failed}");
    }
    eprintln!("\nToken Usage:");
    eprintln!("  Input tokens:  {}", tokens.input);
    eprintln!("  Output tokens: {}", tokens.output);
    eprintln!("  Total tokens:  {}", tokens.total());
    eprintln!("\nCost:");
    eprintln!("  Model: {model}");
    if let Some(p) = pricing::model_pricing(model) {
        eprintln!(
            "  Input cost:  {} (${:.2}/M tokens)",
            pricing::format_cost((tokens.input as f64 / 1_000_000.0) * p.input_cost_per_million),
            p.input_cost_per_million
        );
        eprintln!(
            "  Output cost: {} (${:.2}/M tokens)",
            pricing::format_cost((tokens.output as f64 / 1_000_000.0) * p.output_cost_per_million),
            p.output_cost_per_million
        );
    }
    eprintln!("  Total cost:  {}", pricing::format_cost(cost));
    eprintln!("\nPerformance:");
    eprintln!("  Total time: {:.1}s", elapsed.as_secs_f64());
    if total > 0 {
        let avg = elapsed.as_millis() as f64 / total as f64;
        eprintln!("  Avg time per row: {avg:.0}ms");
    }
}

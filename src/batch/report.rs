use crate::llm::pricing;
use crate::style::style;

/// Field key used to record processing errors on failed rows.
pub const ERROR_FIELD: &str = "_error";

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
    let s = style();

    eprintln!("\n{}", s.dim("─".repeat(50)));
    eprintln!("{}", s.bold("Summary"));
    eprintln!("{}", s.dim("─".repeat(50)));

    eprintln!("\n{}", s.bold("Results"));
    eprintln!("  Processed  {total}");
    eprintln!("  Succeeded  {}", s.success(succeeded));
    if failed > 0 {
        eprintln!("  Failed     {}", s.error_text(failed));
    }

    eprintln!("\n{}", s.bold("Tokens"));
    eprintln!("  Input      {}", tokens.input);
    eprintln!("  Output     {}", tokens.output);
    eprintln!("  Total      {}", tokens.total());

    eprintln!("\n{}", s.bold("Cost"));
    eprintln!("  Model      {model}");
    if let Some(p) = pricing::model_pricing(model) {
        let cost = pricing::calculate_cost(model, tokens.input, tokens.output);
        eprintln!(
            "  Input      {} {}",
            pricing::format_cost((tokens.input as f64 / 1_000_000.0) * p.input_cost_per_million),
            s.muted(format!("(${:.2}/M)", p.input_cost_per_million))
        );
        eprintln!(
            "  Output     {} {}",
            pricing::format_cost((tokens.output as f64 / 1_000_000.0) * p.output_cost_per_million),
            s.muted(format!("(${:.2}/M)", p.output_cost_per_million))
        );
        eprintln!("  Total      {}", s.accent(pricing::format_cost(cost)));
    } else {
        eprintln!("  {}", s.muted("(pricing unavailable for this model)"));
    }

    eprintln!("\n{}", s.bold("Performance"));
    eprintln!("  Time       {:.1}s", elapsed.as_secs_f64());
    if total > 0 {
        let avg = elapsed.as_millis() as f64 / total as f64;
        eprintln!("  Avg/row    {avg:.0}ms");
    }
}

use std::sync::mpsc;

use indicatif::{ProgressBar, ProgressStyle};

use crate::llm::pricing;
use crate::style::style;

use super::events::{BatchEvent, BatchSummary, RowState};

/// Consume BatchEvents and render via indicatif progress bar.
/// Prints the end-of-run summary to stderr.
pub fn run_plain_renderer(rx: mpsc::Receiver<BatchEvent>, total: usize) {
    let pb = ProgressBar::new(total as u64);
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.cyan} [{elapsed_precise}] {bar:28.cyan/dim} {pos}/{len}  {msg}",
        )
        .unwrap()
        .progress_chars("━━─")
        .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ "),
    );

    for event in rx {
        match event {
            BatchEvent::RowStateChanged(update) => {
                if matches!(
                    update.state,
                    RowState::Succeeded | RowState::Failed { .. }
                ) {
                    pb.inc(1);
                }
            }
            BatchEvent::CostUpdate { cost, .. } => {
                pb.set_message(pricing::format_cost(cost));
            }
            BatchEvent::Log(msg) => {
                let s = style();
                // Format retry messages like the old engine did
                if msg.starts_with("Retry ") {
                    pb.println(format!(
                        "  {}",
                        s.yellow(&msg)
                    ));
                } else {
                    pb.println(&msg);
                }
            }
            BatchEvent::RunDone(summary) => {
                pb.finish_and_clear();
                print_plain_summary(&summary);
                if summary.interrupted {
                    let s = style();
                    eprintln!(
                        "{}",
                        s.yellow("Interrupted by user. Partial results saved.")
                    );
                }
            }
            BatchEvent::Fatal(msg) => {
                pb.finish_and_clear();
                eprintln!("Fatal: {msg}");
            }
        }
    }
}

fn print_plain_summary(summary: &BatchSummary) {
    let s = style();
    let total = summary.succeeded + summary.failed;

    eprintln!("\n{}", s.dim("─".repeat(50)));
    eprintln!("{}", s.bold("Summary"));
    eprintln!("{}", s.dim("─".repeat(50)));

    eprintln!("\n{}", s.bold("Results"));
    eprintln!("  Processed  {total}");
    eprintln!("  Succeeded  {}", s.success(summary.succeeded));
    if summary.failed > 0 {
        eprintln!("  Failed     {}", s.error_text(summary.failed));
    }

    eprintln!("\n{}", s.bold("Tokens"));
    eprintln!("  Input      {}", summary.input_tokens);
    eprintln!("  Output     {}", summary.output_tokens);
    eprintln!(
        "  Total      {}",
        summary.input_tokens + summary.output_tokens
    );

    eprintln!("\n{}", s.bold("Cost"));
    eprintln!("  Model      {}", summary.model);
    if let Some(p) = pricing::model_pricing(&summary.model) {
        eprintln!(
            "  Input      {} {}",
            pricing::format_cost(
                (summary.input_tokens as f64 / 1_000_000.0) * p.input_cost_per_million
            ),
            s.muted(format!("(${:.2}/M)", p.input_cost_per_million))
        );
        eprintln!(
            "  Output     {} {}",
            pricing::format_cost(
                (summary.output_tokens as f64 / 1_000_000.0) * p.output_cost_per_million
            ),
            s.muted(format!("(${:.2}/M)", p.output_cost_per_million))
        );
    }
    eprintln!("  Total      {}", s.accent(pricing::format_cost(summary.cost)));

    eprintln!("\n{}", s.bold("Performance"));
    eprintln!("  Time       {:.1}s", summary.elapsed.as_secs_f64());
    if total > 0 {
        let avg = summary.elapsed.as_millis() as f64 / total as f64;
        eprintln!("  Avg/row    {avg:.0}ms");
    }
}

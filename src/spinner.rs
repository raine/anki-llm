use std::time::Duration;

use indicatif::{ProgressBar, ProgressStyle};

/// Create a spinner that displays while waiting for an LLM response.
/// Call `finish_and_clear()` when done.
pub fn llm_spinner(message: impl Into<String>) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("{spinner:.cyan} {msg}")
            .unwrap()
            .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ "),
    );
    pb.set_message(message.into());
    pb.enable_steady_tick(Duration::from_millis(80));
    pb
}

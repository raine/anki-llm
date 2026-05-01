use anyhow::Result;

use crate::cli::GenerateArgs;

pub use super::tui_adapter::run_pipeline;

/// Entry point: dispatch to TUI or legacy mode.
pub fn run(args: GenerateArgs) -> Result<()> {
    use std::io::IsTerminal;

    if args.copy || !std::io::stdout().is_terminal() {
        super::legacy_adapter::run_legacy(args)
    } else {
        super::tui::run_tui(args)
    }
}

use anyhow::Result;
use clap::Parser;

mod app;

mod batch;
mod terminal;

// Re-export shared modules from the library crate so the binary's private
// modules (batch, …) can reach them via `crate::anki`, `crate::llm`, etc.
use anki_llm::anki;
use anki_llm::cli;
use anki_llm::config;
use anki_llm::data;
use anki_llm::generate;
use anki_llm::llm;
use anki_llm::snapshot;
use anki_llm::style;
use anki_llm::template;
use anki_llm::workspace;

fn main() -> Result<()> {
    let cli = cli::Cli::parse();
    app::run(cli)
}

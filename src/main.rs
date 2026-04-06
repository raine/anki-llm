use anyhow::Result;
use clap::Parser;

mod app;

mod batch;
mod config;
mod generate;
#[allow(dead_code)]
mod llm;
mod template;
mod terminal;

// Re-export shared modules from the library crate so the binary's private
// modules (app, config, …) can reach them via `crate::anki` / `crate::data`.
use anki_llm::anki;
use anki_llm::cli;
use anki_llm::data;
use anki_llm::spinner;

fn main() -> Result<()> {
    let cli = cli::Cli::parse();
    app::run(cli)
}

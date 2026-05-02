use anyhow::Result;
use clap::Parser;

fn main() -> Result<()> {
    let cli = anki_llm::cli::Cli::parse();
    anki_llm::run_cli(cli)
}

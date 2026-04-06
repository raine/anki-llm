use anyhow::Result;
use clap::Parser;
use clap::builder::styling::{AnsiColor, Effects, Styles};

mod app;
mod cli;

mod anki;
mod batch;
mod config;
mod data;
mod generate;
#[allow(dead_code)]
mod llm;
mod template;
mod terminal;

const STYLES: Styles = Styles::styled()
    .header(AnsiColor::Green.on_default().effects(Effects::BOLD))
    .usage(AnsiColor::Green.on_default().effects(Effects::BOLD))
    .literal(AnsiColor::Cyan.on_default().effects(Effects::BOLD))
    .placeholder(AnsiColor::Cyan.on_default());

fn main() -> Result<()> {
    let cli = cli::Cli::parse();
    app::run(cli)
}

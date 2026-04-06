use anyhow::Result;

use crate::cli::{Cli, Commands};

pub fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Commands::Config(args) => crate::config::command::run(args.action),
        Commands::Query(args) => crate::anki::command::run(args),
        Commands::Export(args) => crate::data::command_export::run(args),
        Commands::Import(args) => crate::data::command_import::run(args),
        Commands::ProcessFile(args) => crate::batch::command_process_file::run(args),
        Commands::ProcessDeck(args) => crate::batch::command_process_deck::run(args),
        Commands::Generate(args) => crate::generate::command_generate::run(args),
        Commands::GenerateInit(args) => crate::generate::init::command::run(args),
    }
}

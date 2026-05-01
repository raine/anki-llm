use anyhow::Result;

use crate::cli::{Cli, Commands};

pub fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Commands::Config(args) => crate::config::command::run(args.action),
        Commands::Query(args) => crate::anki::command::run(args),
        Commands::Export(args) => crate::data::export::command::run(args),
        Commands::Import(args) => crate::data::import::command::run(args),
        Commands::ProcessFile(args) => crate::batch::process_file::command::run(args),
        Commands::ProcessDeck(args) => crate::batch::process_deck::command::run(args),
        Commands::Generate(args) => crate::generate::command::run(args),
        Commands::GenerateInit(args) => crate::generate::init::command::run(args),
        Commands::History => crate::snapshot::history::command::run(),
        Commands::Rollback(args) => crate::snapshot::rollback::command::run(args),
        Commands::Tts(args) => crate::tts::command::run(args),
        Commands::TtsVoices(args) => crate::tts::voices::command::run(args),
        Commands::Update => crate::update::run(),
        Commands::Docs => crate::docs::run(),
        Commands::Workspace(args) => crate::workspace::command::run(args.action),
        Commands::NoteType(args) => crate::note_type::command::run(args.action),
        Commands::Doctor(args) => crate::doctor::command::run(args),
    }
}

use anyhow::{Result, bail};

use crate::cli::{Cli, Commands};

pub fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Commands::Config(args) => crate::config::command::run(args.action),
        Commands::Query(args) => crate::anki::command::run(args),
        Commands::Export(_) => bail!("export not implemented"),
        Commands::Import(_) => bail!("import not implemented"),
        Commands::ProcessFile(_) => bail!("process-file not implemented"),
        Commands::ProcessDeck(_) => bail!("process-deck not implemented"),
        Commands::Generate(_) => bail!("generate not implemented"),
        Commands::GenerateInit(_) => bail!("generate-init not implemented"),
    }
}

use std::path::PathBuf;

use clap::Parser;
use clap::builder::styling::{AnsiColor, Effects, Styles};

const STYLES: Styles = Styles::styled()
    .header(AnsiColor::Green.on_default().effects(Effects::BOLD))
    .usage(AnsiColor::Green.on_default().effects(Effects::BOLD))
    .literal(AnsiColor::Cyan.on_default().effects(Effects::BOLD))
    .placeholder(AnsiColor::Cyan.on_default());

#[derive(Parser)]
#[command(name = "anki-llm")]
#[command(about = "Bulk-process Anki flashcards with LLMs")]
#[command(styles = STYLES)]
#[command(version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(clap::Subcommand)]
pub enum Commands {
    /// Export an Anki deck to CSV or YAML
    Export(ExportArgs),
    /// Import CSV or YAML file into an Anki deck
    Import(ImportArgs),
    /// Process notes from a file with AI (supports resume)
    #[command(name = "process-file")]
    ProcessFile(ProcessFileArgs),
    /// Process notes directly from an Anki deck
    #[command(name = "process-deck")]
    ProcessDeck(ProcessDeckArgs),
    /// Query AnkiConnect API
    Query(QueryArgs),
    /// Manage persistent configuration
    Config(ConfigArgs),
    /// Generate Anki cards using an LLM
    Generate(GenerateArgs),
    /// Create a prompt template by querying your Anki collection
    #[command(name = "generate-init")]
    GenerateInit(GenerateInitArgs),
}

#[derive(clap::Args)]
pub struct ExportArgs {
    /// Deck name to export
    pub deck: String,
    /// Output file path
    pub output: Option<PathBuf>,
    /// Filter by note type (required if deck contains multiple note types)
    #[arg(long, short = 'n')]
    pub note_type: Option<String>,
}

#[derive(clap::Args)]
pub struct ImportArgs {
    /// Input file path (CSV or YAML)
    pub input: PathBuf,

    /// Target Anki deck name
    #[arg(long, short = 'd')]
    pub deck: String,

    /// Anki note type name (inferred from deck if not specified)
    #[arg(long, short = 'n')]
    pub note_type: Option<String>,

    /// Field name to use for identifying existing notes (auto-detected if not specified)
    #[arg(long, short = 'k')]
    pub key_field: Option<String>,
}

#[derive(clap::Args)]
pub struct ProcessFileArgs {
    /// Input file path
    pub input: PathBuf,
}

#[derive(clap::Args)]
pub struct ProcessDeckArgs {
    /// Deck name to process
    pub deck: String,
}

#[derive(clap::Args)]
pub struct QueryArgs {
    /// AnkiConnect action name
    pub action: String,
    /// JSON parameters
    pub params: Option<String>,
}

#[derive(clap::Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub action: ConfigAction,
}

#[derive(clap::Subcommand)]
pub enum ConfigAction {
    /// Get a configuration value
    Get {
        /// Config key
        key: String,
    },
    /// Set a configuration value
    Set {
        /// Config key
        key: String,
        /// Config value
        value: String,
    },
    /// List all configuration settings
    List,
    /// Show the config file path
    Path,
}

#[derive(clap::Args)]
pub struct GenerateArgs {
    /// Term to generate cards for
    pub term: String,
}

#[derive(clap::Args)]
pub struct GenerateInitArgs {
    /// Output file path
    pub output: Option<PathBuf>,
}

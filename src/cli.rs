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
#[command(group(
    clap::ArgGroup::new("output_mode")
        .required(true)
        .args(["field", "json"])
        .multiple(false)
))]
pub struct ProcessFileArgs {
    /// Input file path (CSV or YAML)
    pub input: PathBuf,

    /// Path to prompt template file
    #[arg(long, short = 'p')]
    pub prompt: PathBuf,

    /// Output file path (CSV or YAML)
    #[arg(long, short = 'o')]
    pub output: PathBuf,

    /// Field name to update with LLM response (mutually exclusive with --json)
    #[arg(long)]
    pub field: Option<String>,

    /// Expect JSON response and merge fields case-insensitively (mutually exclusive with --field)
    #[arg(long)]
    pub json: bool,

    /// Model name
    #[arg(long, short = 'm')]
    pub model: Option<String>,

    /// Number of concurrent requests
    #[arg(long, short = 'b', default_value = "5")]
    pub batch_size: u32,

    /// Sampling temperature (0-2)
    #[arg(long, short = 't')]
    pub temperature: Option<f64>,

    /// Maximum tokens per completion
    #[arg(long)]
    pub max_tokens: Option<u64>,

    /// Number of retries on failure
    #[arg(long, short = 'r', default_value = "3")]
    pub retries: u32,

    /// Re-process all rows, ignoring existing output
    #[arg(long, short = 'f')]
    pub force: bool,

    /// Preview without making API calls
    #[arg(long, short = 'd')]
    pub dry_run: bool,

    /// Limit the number of rows to process
    #[arg(long)]
    pub limit: Option<usize>,

    /// Require <result></result> tags in LLM responses
    #[arg(long)]
    pub require_result_tag: bool,
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

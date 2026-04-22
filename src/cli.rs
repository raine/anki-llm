use std::path::PathBuf;

use clap::Parser;

pub const DEFAULT_BATCH_SIZE: u32 = 5;
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
    /// List past process-deck runs with snapshot data
    History,
    /// Rollback a previous process-deck run
    Rollback(RollbackArgs),
    /// Generate text-to-speech audio for notes and upload to Anki's media store
    Tts(TtsArgs),
    /// Browse and audition TTS voices across OpenAI / Azure / Google / Polly
    #[command(name = "tts-voices")]
    TtsVoices(VoicesArgs),
    /// Update anki-llm to the latest version
    Update,
}

#[derive(clap::Args)]
pub struct VoicesArgs {
    /// Pre-filter by language code prefix, e.g. "ja" or "en-US".
    #[arg(long)]
    pub lang: Option<String>,

    /// Pre-filter by provider id (openai, azure, google, amazon).
    #[arg(long)]
    pub provider: Option<String>,

    /// Initial text query for the omni-search field.
    #[arg(long, short = 'q')]
    pub query: Option<String>,
}

#[derive(clap::Args)]
pub struct TtsArgs {
    /// Deck name to process (defaults to deck from --prompt frontmatter)
    pub deck: Option<String>,

    /// Anki search query (e.g. "tag:leech", "deck:Japanese -Audio:")
    #[arg(long, short = 'q')]
    pub query: Option<String>,

    /// Path to a generate prompt YAML; reads its `tts:` block instead of
    /// taking deck-design flags on the CLI
    #[arg(long)]
    pub prompt: Option<PathBuf>,

    /// Target field to write [sound:...] into (flag mode; required)
    #[arg(long)]
    pub field: Option<String>,

    /// Path to prompt template file (flag mode)
    #[arg(long, short = 'p')]
    pub template: Option<PathBuf>,

    /// Source field name (flag mode; alternative to --template)
    #[arg(long = "text-field")]
    pub text_field: Option<String>,

    /// TTS provider identifier (flag mode; defaults to "openai")
    #[arg(long)]
    pub provider: Option<String>,

    /// Voice name (flag mode; provider-specific, e.g. alloy)
    #[arg(long)]
    pub voice: Option<String>,

    /// TTS backing model (flag mode; e.g. gpt-4o-mini-tts)
    #[arg(long = "tts-model")]
    pub tts_model: Option<String>,

    /// Output audio format (flag mode; defaults to "mp3")
    #[arg(long)]
    pub format: Option<String>,

    /// Playback speed (flag mode; 1.0 = normal)
    #[arg(long)]
    pub speed: Option<f32>,

    /// API base URL override (OpenAI or OpenAI-compatible providers)
    #[arg(long)]
    pub api_base_url: Option<String>,

    /// API key override. Used as the OpenAI bearer token or the Azure
    /// subscription key depending on the active provider.
    #[arg(long)]
    pub api_key: Option<String>,

    /// Azure region override (flag mode; provider must be 'azure')
    #[arg(long = "azure-region")]
    pub azure_region: Option<String>,

    /// AWS region override for Amazon Polly (flag mode; provider must be 'amazon')
    #[arg(long = "aws-region")]
    pub aws_region: Option<String>,

    /// AWS access key id for Amazon Polly (flag mode)
    #[arg(long = "aws-access-key-id")]
    pub aws_access_key_id: Option<String>,

    /// AWS secret access key for Amazon Polly (flag mode)
    #[arg(long = "aws-secret-access-key")]
    pub aws_secret_access_key: Option<String>,

    /// Filter by note type (flag mode; rejected in --prompt mode)
    #[arg(long, short = 'n')]
    pub note_type: Option<String>,

    /// Number of concurrent TTS requests
    #[arg(long, short = 'b', default_value_t = DEFAULT_BATCH_SIZE, value_parser = clap::value_parser!(u32).range(1..))]
    pub batch_size: u32,

    /// Number of retries on transient failures
    #[arg(long, short = 'r', default_value = "3")]
    pub retries: u32,

    /// Regenerate audio even for notes whose target field already contains a sound tag
    #[arg(long, short = 'f')]
    pub force: bool,

    /// Preview without making API calls or mutating Anki
    #[arg(long, short = 'd')]
    pub dry_run: bool,

    /// Limit the number of notes to process
    #[arg(long)]
    pub limit: Option<usize>,
}

#[derive(clap::Args)]
#[command(group(
    clap::ArgGroup::new("source")
        .required(true)
        .args(["deck", "query"])
        .multiple(false)
))]
pub struct ExportArgs {
    /// Deck name to export
    pub deck: Option<String>,
    /// Anki search query (e.g. "tag:leech", "prop:lapses>3", "deck:Japanese -field:Audio")
    #[arg(long, short = 'q')]
    pub query: Option<String>,
    /// Output file path
    #[arg(long, short = 'o')]
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

    /// Custom API base URL (e.g. https://openrouter.ai/api/v1, http://localhost:11434/v1)
    #[arg(long)]
    pub api_base_url: Option<String>,

    /// API key (overrides environment variables)
    #[arg(long)]
    pub api_key: Option<String>,

    /// Number of concurrent requests
    #[arg(long, short = 'b', default_value_t = DEFAULT_BATCH_SIZE, value_parser = clap::value_parser!(u32).range(1..))]
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

    /// Append raw LLM prompts and responses to a log file (relative path)
    #[arg(long)]
    pub log: Option<PathBuf>,

    /// Print raw LLM prompts and responses to stderr
    #[arg(long)]
    pub very_verbose: bool,
}

#[derive(clap::Args)]
#[command(group(
    clap::ArgGroup::new("output_mode")
        .required(true)
        .args(["field", "json"])
        .multiple(false)
))]
#[command(group(
    clap::ArgGroup::new("source")
        .required(true)
        .args(["deck", "query"])
        .multiple(false)
))]
pub struct ProcessDeckArgs {
    /// Deck name to process
    pub deck: Option<String>,

    /// Anki search query (e.g. "tag:leech", "prop:lapses>3", "deck:Japanese prop:lapses>5")
    #[arg(long, short = 'q')]
    pub query: Option<String>,

    /// Path to prompt template file
    #[arg(long, short = 'p')]
    pub prompt: PathBuf,

    /// Field name to update with LLM response (mutually exclusive with --json)
    #[arg(long)]
    pub field: Option<String>,

    /// Expect JSON response and merge fields case-insensitively (mutually exclusive with --field)
    #[arg(long)]
    pub json: bool,

    /// Filter by note type (required if deck contains multiple note types)
    #[arg(long, short = 'n')]
    pub note_type: Option<String>,

    /// Model name
    #[arg(long, short = 'm')]
    pub model: Option<String>,

    /// Custom API base URL (e.g. https://openrouter.ai/api/v1, http://localhost:11434/v1)
    #[arg(long)]
    pub api_base_url: Option<String>,

    /// API key (overrides environment variables)
    #[arg(long)]
    pub api_key: Option<String>,

    /// Number of concurrent requests
    #[arg(long, short = 'b', default_value_t = DEFAULT_BATCH_SIZE, value_parser = clap::value_parser!(u32).range(1..))]
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

    /// Preview without making API calls
    #[arg(long, short = 'd')]
    pub dry_run: bool,

    /// Limit the number of notes to process
    #[arg(long)]
    pub limit: Option<usize>,

    /// Require <result></result> tags in LLM responses
    #[arg(long)]
    pub require_result_tag: bool,

    /// Append raw LLM prompts and responses to a log file (relative path)
    #[arg(long)]
    pub log: Option<PathBuf>,

    /// Print raw LLM prompts and responses to stderr
    #[arg(long)]
    pub very_verbose: bool,
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
    /// Term to generate cards for (omit to enter interactively in TUI)
    pub term: Option<String>,

    /// Path to prompt template file with frontmatter (auto-resolved from prompts_dir if omitted)
    #[arg(long, short = 'p')]
    pub prompt: Option<PathBuf>,

    /// Number of card examples to generate
    #[arg(long, short = 'c', default_value = "3")]
    pub count: u32,

    /// Model name
    #[arg(long, short = 'm')]
    pub model: Option<String>,

    /// Custom API base URL (e.g. https://openrouter.ai/api/v1, http://localhost:11434/v1)
    #[arg(long)]
    pub api_base_url: Option<String>,

    /// API key (overrides environment variables)
    #[arg(long)]
    pub api_key: Option<String>,

    /// Preview without importing to Anki
    #[arg(long, short = 'd')]
    pub dry_run: bool,

    /// Number of retries for failed requests
    #[arg(long, short = 'r', default_value = "3")]
    pub retries: u32,

    /// Maximum tokens per response
    #[arg(long)]
    pub max_tokens: Option<u64>,

    /// LLM temperature (0-2)
    #[arg(long, short = 't')]
    pub temperature: Option<f64>,

    /// Export cards to a file instead of importing to Anki
    #[arg(long, short = 'o')]
    pub output: Option<PathBuf>,

    /// Copy prompt to clipboard for manual LLM mode
    #[arg(long)]
    pub copy: bool,

    /// Append raw LLM prompts and responses to a log file (relative path)
    #[arg(long)]
    pub log: Option<PathBuf>,

    /// Print raw LLM prompts and responses to stderr
    #[arg(long)]
    pub very_verbose: bool,
}

#[derive(clap::Args)]
pub struct GenerateInitArgs {
    /// Output file path
    pub output: Option<PathBuf>,

    /// Model name
    #[arg(long, short = 'm')]
    pub model: Option<String>,

    /// Custom API base URL (e.g. https://openrouter.ai/api/v1, http://localhost:11434/v1)
    #[arg(long)]
    pub api_base_url: Option<String>,

    /// API key (overrides environment variables)
    #[arg(long)]
    pub api_key: Option<String>,

    /// LLM temperature (0-2)
    #[arg(long, short = 't')]
    pub temperature: Option<f64>,

    /// Copy prompt to clipboard for manual LLM mode
    #[arg(long)]
    pub copy: bool,
}

#[derive(clap::Args)]
pub struct RollbackArgs {
    /// Run ID to rollback (from `history` command)
    pub run_id: String,

    /// Force rollback even if notes were modified after the run
    #[arg(long)]
    pub force: bool,

    /// Preview without making changes
    #[arg(long, short = 'd')]
    pub dry_run: bool,
}

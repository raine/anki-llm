use anyhow::Result;

use crate::anki::client::{AnkiClient, anki_client};
use crate::cli::GenerateArgs;
use crate::llm::client::LlmClient;
use crate::llm::logger::LlmLogger;
use crate::llm::runtime::{RuntimeConfig, RuntimeConfigArgs, build_runtime_config};
use crate::template::frontmatter::{Frontmatter, parse_prompt_file};
use crate::tts::service::SessionTts;

use super::pipeline::{PipelineProgress, PipelineStep};
use super::validate::{ValidationResult, validate_anki_assets};

/// Resolve `args.prompt` to a concrete path for non-interactive use (legacy mode, worker thread).
pub(super) fn require_prompt_path(
    prompt: &Option<std::path::PathBuf>,
) -> Result<std::path::PathBuf> {
    crate::workspace::resolver::resolve_prompt_path(prompt.clone())
}

pub(super) struct LoadedPrompt {
    pub frontmatter: Frontmatter,
    pub body: String,
}

pub(super) struct PreparedSession {
    pub frontmatter: Frontmatter,
    pub prompt_body: String,
    pub validation: ValidationResult,
    pub runtime: RuntimeConfig,
    pub field_map_keys: Vec<String>,
    pub anki: AnkiClient,
    pub client: LlmClient,
    pub logger: LlmLogger,
    /// Lazy TTS bundle. Holds the `TtsSpec` from the prompt frontmatter
    /// and a `OnceCell<TtsBundle>` that's populated on first access via
    /// `SessionTts::bundle()`. Deferring construction means
    /// `--dry-run` / `--output` and sessions that never press `p` never
    /// trigger TTS credential resolution — previously `prepare_session`
    /// would fail at startup with "Azure TTS requires a subscription
    /// key" for any prompt declaring a `tts:` block, regardless of
    /// whether the run actually needed audio.
    pub tts: Option<SessionTts>,
}

/// Load and parse a prompt file, validating required placeholders.
pub(super) fn load_prompt(args: &GenerateArgs) -> Result<LoadedPrompt> {
    let prompt_path = require_prompt_path(&args.prompt)?;
    crate::workspace::resolver::save_last_prompt(&prompt_path);
    let content = std::fs::read_to_string(&prompt_path)?;
    let parsed = parse_prompt_file(&content)?;

    if !parsed.body.contains("{term}") {
        anyhow::bail!("Prompt is missing required placeholder: {{term}}");
    }
    if !parsed.body.contains("{count}") {
        anyhow::bail!("Prompt is missing required placeholder: {{count}}");
    }

    Ok(LoadedPrompt {
        frontmatter: parsed.frontmatter,
        body: parsed.body,
    })
}

/// Full session setup: load prompt, validate Anki, build logger/runtime/client.
/// Reports progress via the `PipelineProgress` trait (TUI shows steps, legacy no-ops).
pub(super) fn prepare_session(
    args: &GenerateArgs,
    very_verbose: bool,
    progress: &dyn PipelineProgress,
) -> Result<PreparedSession> {
    progress.step_start(PipelineStep::LoadPrompt, None);
    let loaded = load_prompt(args).inspect_err(|e| {
        progress.step_error(PipelineStep::LoadPrompt, &e.to_string());
    })?;
    progress.step_done(PipelineStep::LoadPrompt, None);

    progress.step_start(PipelineStep::ValidateAnki, None);
    let anki = anki_client();
    let validation = validate_anki_assets(&anki, &loaded.frontmatter).inspect_err(|e| {
        progress.step_error(PipelineStep::ValidateAnki, &e.to_string());
    })?;
    progress.step_done(PipelineStep::ValidateAnki, None);

    let logger = LlmLogger::new(args.log.as_deref(), very_verbose)?;
    let runtime = build_runtime_config(RuntimeConfigArgs {
        model: args.model.as_deref(),
        api_base_url: args.api_base_url.as_deref(),
        api_key: args.api_key.as_deref(),
        batch_size: None,
        max_tokens: args.max_tokens,
        temperature: args.temperature,
        retries: args.retries,
        dry_run: false,
    })?;
    let field_map_keys: Vec<String> = loaded.frontmatter.field_map.keys().cloned().collect();
    let client = LlmClient::from_config(&runtime);

    // TTS bundle is built lazily on first access via `SessionTts::bundle()`
    // so `--dry-run` / `--output` — and sessions where the user never
    // triggers preview or import — don't pay the credential-resolution
    // cost. Credentials flow through env/config
    // (`AZURE_TTS_KEY`, `OPENAI_API_KEY`,
    // `~/.config/anki-llm/config.json`'s `tts_*` fields); generate's
    // `--api-key` / `--api-base-url` are LLM-only and never forwarded.
    let tts = loaded.frontmatter.tts.clone().map(SessionTts::new);

    Ok(PreparedSession {
        frontmatter: loaded.frontmatter,
        prompt_body: loaded.body,
        validation,
        runtime,
        field_map_keys,
        anki,
        client,
        logger,
        tts,
    })
}

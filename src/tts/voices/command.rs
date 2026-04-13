//! `anki-llm tts-voices` entry point — launches the ratatui browser.

use std::sync::Arc;

use anyhow::{Context, Result};

use crate::cli::VoicesArgs;
use crate::tts::cache::TtsCache;
use crate::tui::terminal;

use super::app::{self, InitialFilters};
use super::catalog::ProviderId;

pub fn run(args: VoicesArgs) -> Result<()> {
    let provider_filter = match args.provider.as_deref() {
        Some(s) => Some(ProviderId::parse(s).with_context(|| {
            format!("unknown provider '{s}' (expected: openai, azure, google, amazon)")
        })?),
        None => None,
    };
    let filters = InitialFilters {
        lang: args.lang,
        provider: provider_filter,
        query: args.query,
    };

    let cache_dir = TtsCache::default_dir()
        .context("failed to locate TTS cache directory (home dir unavailable)")?;
    let cache = Arc::new(TtsCache::new(cache_dir).context("failed to initialize TTS cache")?);

    let terminal = terminal::init();
    let outcome = app::run(terminal, filters, cache);
    terminal::restore();

    if let Some(out) = outcome {
        if let Ok(mut cb) = arboard::Clipboard::new() {
            let _ = cb.set_text(out.voice_id.clone());
        }
        print!("{}", out.yaml);
    }
    Ok(())
}

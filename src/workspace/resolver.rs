use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::config::store::{read_config, read_state, write_state};
use crate::workspace::discovery::{PromptEntry, discover_prompts};

/// Result of prompt resolution.
pub enum ResolvedPrompt {
    /// Use this specific prompt file.
    Resolved(PathBuf),
    /// Multiple prompts available and we're in an interactive terminal — show picker.
    ShowPicker(Vec<PromptEntry>),
}

/// Default prompts directory: `~/.config/anki-llm/prompts`.
fn default_prompts_dir() -> Option<PathBuf> {
    home::home_dir().map(|h| h.join(".config").join("anki-llm").join("prompts"))
}

/// Resolve the effective prompts directory from config (or default).
fn effective_prompts_dir() -> Result<PathBuf> {
    let config = read_config().context("failed to read config")?;
    let dir = config.prompts_dir.or_else(default_prompts_dir);
    let Some(dir) = dir else {
        bail!(
            "No prompt specified and could not determine prompts directory.\n\
             Either pass --prompt <path> or set a prompts directory:\n  \
             anki-llm config set prompts_dir ~/anki-prompts"
        );
    };
    Ok(dir)
}

/// Resolve which prompt file to use.
///
/// - If `explicit` is `Some`, return it directly.
/// - Otherwise, look up prompts_dir (config override or `~/.config/anki-llm/prompts`).
/// - In interactive mode with multiple prompts, returns `ShowPicker`.
/// - In non-interactive mode, falls back to last-used or single-prompt.
pub fn resolve_prompt(explicit: Option<PathBuf>, force_picker: bool) -> Result<ResolvedPrompt> {
    if let Some(path) = explicit {
        return Ok(ResolvedPrompt::Resolved(path));
    }

    let prompts_dir = effective_prompts_dir()?;

    if !prompts_dir.is_dir() {
        bail!(
            "Prompts directory does not exist: {}\n\
             Create it with: mkdir -p {}",
            prompts_dir.display(),
            prompts_dir.display()
        );
    }

    let prompts = discover_prompts(&prompts_dir);

    if prompts.is_empty() {
        bail!(
            "No prompt files found in {}\n\
             Create one with: anki-llm generate-init",
            prompts_dir.display()
        );
    }

    let is_interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();

    // Interactive: use last-used if still valid, otherwise show picker
    if is_interactive {
        if !force_picker {
            let state = read_state().unwrap_or_default();
            if let Some(ref last) = state.last_prompt
                && last.exists()
                && prompts.iter().any(|p| p.path == *last)
            {
                return Ok(ResolvedPrompt::Resolved(last.clone()));
            }
        }
        return Ok(ResolvedPrompt::ShowPicker(prompts));
    }

    // Non-interactive: try last-used, then single prompt, then error
    let state = read_state().unwrap_or_default();
    if let Some(ref last) = state.last_prompt
        && last.exists()
    {
        return Ok(ResolvedPrompt::Resolved(last.clone()));
    }

    if prompts.len() == 1 {
        return Ok(ResolvedPrompt::Resolved(
            prompts.into_iter().next().unwrap().path,
        ));
    }

    let names: Vec<_> = prompts.iter().map(|p| format!("  - {}", p.title)).collect();
    bail!(
        "Multiple prompts found and no --prompt specified.\n\
         Available prompts:\n{}\n\
         Use --prompt <path> to select one.",
        names.join("\n")
    );
}

/// Resolve prompt path for non-interactive commands (batch).
///
/// If `explicit` is `Some`, return it. Otherwise resolve from prompts_dir:
/// single prompt → last-used → error.
pub fn resolve_prompt_path(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path);
    }

    let prompts_dir = effective_prompts_dir()?;

    if !prompts_dir.is_dir() {
        bail!(
            "Prompts directory does not exist: {}",
            prompts_dir.display()
        );
    }

    let prompts = discover_prompts(&prompts_dir);

    if prompts.is_empty() {
        bail!("No prompt files found in {}", prompts_dir.display());
    }

    if prompts.len() == 1 {
        return Ok(prompts.into_iter().next().unwrap().path);
    }

    // Try last-used prompt
    let state = read_state().unwrap_or_default();
    if let Some(ref last) = state.last_prompt
        && last.exists()
    {
        return Ok(last.clone());
    }

    let names: Vec<_> = prompts.iter().map(|p| format!("  - {}", p.title)).collect();
    bail!(
        "Multiple prompts found and no --prompt specified.\n\
         Available prompts:\n{}\n\
         Use --prompt <path> to select one.",
        names.join("\n")
    );
}

/// Save the selected prompt as the last-used prompt.
pub fn save_last_prompt(path: &Path) {
    let mut state = read_state().unwrap_or_default();
    state.last_prompt = Some(path.to_path_buf());
    write_state(&state).ok();
}

/// Get the last-used prompt path, if any.
pub fn last_prompt() -> Option<PathBuf> {
    read_state().ok().and_then(|s| s.last_prompt)
}

/// Return the effective prompts directory (for generate-init default output).
pub fn prompts_dir() -> Option<PathBuf> {
    effective_prompts_dir().ok()
}

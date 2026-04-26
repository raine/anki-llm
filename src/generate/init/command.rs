use std::io::IsTerminal;
use std::path::PathBuf;

use anyhow::Result;
use inquire::{Confirm, Select};

use crate::anki::client::anki_client;
use crate::cli::GenerateInitArgs;
use crate::data::slug::slugify_deck_name;
use crate::llm::client::LlmClient;
use crate::llm::runtime::{RuntimeConfigArgs, build_runtime_config};
use crate::template::frontmatter::{Frontmatter, ProcessingConfig, ProcessorKind, ProcessorStep};

use super::interactive::{
    FieldMap, configure_field_mapping, map_inquire_err, select_deck, select_note_type,
};
use super::prompt::{PromptDraft, create_prompt_content, generate_generic_prompt_body};

pub fn run(args: GenerateInitArgs) -> Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!("generate-init requires an interactive terminal (TTY)");
    }

    match run_wizard(args) {
        Ok(()) => Ok(()),
        Err(e) if is_cancelled(&e) => {
            eprintln!("Wizard cancelled by user.");
            Ok(())
        }
        Err(e) => Err(e),
    }
}

fn run_wizard(args: GenerateInitArgs) -> Result<()> {
    eprintln!("generate-init: Create a prompt template for your Anki deck");
    eprintln!("This wizard queries your Anki collection to set up a prompt file.\n");

    let anki = anki_client();

    let deck = select_deck(&anki)?;
    let note_type = select_note_type(&anki, &deck)?;
    let field_map = configure_field_mapping(&anki, &note_type)?;

    // Skip API key validation in copy mode — no LLM call will be made.
    let runtime = build_runtime_config(RuntimeConfigArgs {
        model: args.model.as_deref(),
        api_base_url: args.api_base_url.as_deref(),
        api_key: args.api_key.as_deref(),
        batch_size: None,
        max_tokens: None,
        temperature: args.temperature,
        retries: 3,
        dry_run: args.copy,
    })?;
    let llm_client = LlmClient::from_config(&runtime);

    eprintln!("\nAnalyzing your deck to generate a smart prompt...");
    let draft = match create_prompt_content(
        &anki,
        &deck,
        &note_type,
        &field_map,
        &llm_client,
        &runtime.model,
        runtime.temperature,
        args.copy,
    ) {
        Ok(d) => d,
        Err(e) if is_cancelled(&e) => return Err(e),
        Err(e) => {
            eprintln!("Warning: Could not create contextual prompt ({e}). Using generic template.");
            PromptDraft {
                body: generate_generic_prompt_body(&field_map.keys().cloned().collect::<Vec<_>>()),
                final_field_map: field_map.clone(),
            }
        }
    };

    let processing = configure_quality_check(&draft.final_field_map)?;

    let frontmatter = Frontmatter {
        title: None,
        description: None,
        deck: deck.clone(),
        note_type,
        field_map: draft.final_field_map,
        processing,
        tts: None,
    };

    let yaml = serde_yaml::to_string(&frontmatter)?;
    let content = format!("---\n{yaml}---\n\n{}\n", draft.body.trim());

    let output_path: PathBuf = args.output.unwrap_or_else(|| {
        let filename = format!("{}-prompt.md", slugify_deck_name(&deck));
        // Default into prompts_dir if available
        if let Some(dir) = crate::workspace::resolver::prompts_dir()
            && (dir.is_dir() || std::fs::create_dir_all(&dir).is_ok())
        {
            return dir.join(&filename);
        }
        PathBuf::from(filename)
    });

    std::fs::write(&output_path, &content)?;

    eprintln!("\nPrompt file created: {}", output_path.display());
    eprintln!(
        "\nExample usage:\n  anki-llm generate \"your term\" --prompt {}",
        output_path.display()
    );

    Ok(())
}

fn configure_quality_check(field_map: &FieldMap) -> Result<Option<ProcessingConfig>> {
    let want_qc = Confirm::new("Add a quality check step?")
        .with_default(false)
        .with_help_message("Quality checks use an LLM to review generated cards before importing")
        .prompt()
        .map_err(map_inquire_err)?;

    if !want_qc {
        return Ok(None);
    }

    // Show Anki field names (values) but store the JSON key (key) in config.
    let anki_names: Vec<String> = field_map.values().cloned().collect();
    let selected_anki_name = Select::new("Which field to quality-check?", anki_names)
        .prompt()
        .map_err(map_inquire_err)?;
    let field = field_map
        .iter()
        .find(|(_, v)| *v == &selected_anki_name)
        .map(|(k, _)| k.clone())
        .unwrap_or(selected_anki_name);

    let prompt = format!("Review this flashcard for accuracy and quality.\nContent: {{{field}}}");

    Ok(Some(ProcessingConfig {
        pre_select: vec![],
        post_select: vec![ProcessorStep {
            kind: ProcessorKind::Check,
            target: None,
            writes: Vec::new(),
            prompt,
            model: None,
        }],
    }))
}

fn is_cancelled(e: &anyhow::Error) -> bool {
    e.to_string() == "User cancelled"
}

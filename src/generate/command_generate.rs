use anyhow::Result;

use crate::anki::client::AnkiClient;
use crate::cli::GenerateArgs;
use crate::llm::client::LlmClient;
use crate::llm::logger::LlmLogger;
use crate::llm::parse_json::try_parse_json_array;
use crate::llm::pricing;
use crate::llm::runtime::{RuntimeConfigArgs, build_runtime_config};
use crate::template::frontmatter::parse_prompt_file;

use crate::style::style;

use super::anki_import::{import_cards_to_anki, report_import_result};
use super::cards::{ValidatedCard, validate_cards};
use super::exporter::export_cards;
use super::manual::get_llm_response_manually;
use super::processor::{CardCandidate, generate_cards};
use super::quality::perform_quality_check;
use super::sanitize::sanitize_fields;
use super::selector::{display_cards, select_cards};
use super::validate::validate_anki_assets;

pub fn run(args: GenerateArgs) -> Result<()> {
    // 1. Load and parse prompt file
    let content = std::fs::read_to_string(&args.prompt)?;
    let parsed = parse_prompt_file(&content)?;
    let frontmatter = parsed.frontmatter;

    // Validate required placeholders in prompt body
    if !parsed.body.contains("{term}") {
        anyhow::bail!("Prompt is missing required placeholder: {{term}}");
    }
    if !parsed.body.contains("{count}") {
        anyhow::bail!("Prompt is missing required placeholder: {{count}}");
    }

    let s = style();
    eprintln!("  {}  {}", s.muted("Deck     "), s.cyan(&frontmatter.deck));
    eprintln!(
        "  {}  {}",
        s.muted("Note type"),
        s.cyan(&frontmatter.note_type)
    );

    // 2. Validate Anki assets
    let anki = AnkiClient::new();
    let validation = validate_anki_assets(&anki, &frontmatter)?;
    eprintln!(
        "  {}  {}",
        s.muted("Fields   "),
        s.muted(validation.note_type_fields.join(", "))
    );

    // 3. Build logger
    let logger = LlmLogger::new(args.log.as_deref(), args.very_verbose)?;

    // 4. Resolve LLM config
    // dry_run: false here because generate always calls the LLM (dry-run only
    // skips the Anki import step, not the generation). Passing dry_run: true
    // would replace the API key with "dry-run" and cause HTTP 400.
    let runtime = build_runtime_config(RuntimeConfigArgs {
        model: args.model.as_deref(),
        batch_size: None,
        max_tokens: args.max_tokens,
        temperature: args.temperature,
        retries: args.retries,
        dry_run: false,
    })?;

    // 5. Generate cards
    let field_map_keys: Vec<String> = frontmatter.field_map.keys().cloned().collect();
    let mut generation_cost = 0.0;
    let candidates: Vec<CardCandidate>;

    let client = if args.copy {
        None
    } else {
        Some(LlmClient::from_config(&runtime))
    };

    if args.copy {
        // Manual copy-paste mode
        let mut row = crate::data::Row::new();
        row.insert("term".into(), serde_json::Value::String(args.term.clone()));
        row.insert(
            "count".into(),
            serde_json::Value::String(args.count.to_string()),
        );
        let filled = crate::template::fill_template(&parsed.body, &row)?;
        let raw = get_llm_response_manually(&filled)?;

        let parsed_arr = try_parse_json_array(&raw)
            .ok_or_else(|| anyhow::anyhow!("Response is not a valid JSON array"))?;

        let mut skipped = 0;
        candidates = parsed_arr
            .into_iter()
            .filter_map(|obj| {
                let mut fields = std::collections::HashMap::new();
                let mut missing = false;
                for key in &field_map_keys {
                    match obj.get(key) {
                        Some(val) => {
                            fields.insert(key.clone(), val.clone());
                        }
                        None => {
                            eprintln!(
                                "  {}",
                                s.warning(format!(
                                    "Response is missing field \"{key}\". Skipping card."
                                ))
                            );
                            missing = true;
                        }
                    }
                }
                if missing {
                    skipped += 1;
                    None
                } else {
                    Some(CardCandidate { fields })
                }
            })
            .collect();

        if skipped > 0 {
            eprintln!(
                "  {}",
                s.warning(format!("Skipped {skipped} card(s) due to missing fields."))
            );
        }
        eprintln!("  Parsed {} card(s) from response", candidates.len());
    } else {
        let client = client.as_ref().unwrap();

        let spinner = crate::spinner::llm_spinner(format!(
            "Generating {} card(s) for \"{}\" using {}...",
            args.count, args.term, runtime.model
        ));

        let result = generate_cards(
            &args.term,
            &parsed.body,
            args.count,
            &field_map_keys,
            client,
            &runtime.model,
            runtime.temperature,
            runtime.max_tokens,
            runtime.retries,
            Some(&logger),
        )?;
        spinner.finish_and_clear();

        if let Some(ref cost) = result.cost {
            generation_cost = cost.total_cost;
            eprintln!(
                "  {}  {} in / {} out   {}",
                s.muted("Tokens"),
                cost.input_tokens,
                cost.output_tokens,
                s.muted(pricing::format_cost(cost.total_cost))
            );
        }

        candidates = result.cards;

        if candidates.is_empty() {
            anyhow::bail!("No cards were generated");
        }

        eprintln!("  {} card(s) generated", s.green(candidates.len()));

        if candidates.len() != args.count as usize {
            eprintln!(
                "  {}",
                s.warning(format!(
                    "Requested {} cards, received {}",
                    args.count,
                    candidates.len()
                ))
            );
        }
    }

    // 5. Sanitize and validate

    let sanitized_pairs: Vec<_> = candidates
        .into_iter()
        .map(|c| {
            let s = sanitize_fields(&c.fields);
            (c, s)
        })
        .collect();

    let first_field_name = &validation.note_type_fields[0];
    let validated = validate_cards(sanitized_pairs, &frontmatter, first_field_name, &anki)?;

    let dup_count = validated.iter().filter(|c| c.is_duplicate).count();
    if dup_count > 0 {
        eprintln!(
            "  {}",
            s.muted(format!("{dup_count} duplicate(s) already in Anki"))
        );
    }

    // 6. Select cards
    if args.dry_run {
        display_cards(&validated);
        return Ok(());
    }

    if validated.is_empty() {
        eprintln!("No cards to select from.");
        return Ok(());
    }

    let selected_indices = select_cards(&validated)?;
    let mut selected: Vec<ValidatedCard> = selected_indices
        .iter()
        .filter_map(|&i| validated.get(i).cloned())
        .collect();

    if selected.is_empty() {
        eprintln!("\nNo cards selected. Exiting.");
        return Ok(());
    }

    // Filter out duplicates — they are shown in the selector as a heads-up,
    // but adding them to Anki would create duplicate notes.
    let dup_selected = selected.iter().filter(|c| c.is_duplicate).count();
    if dup_selected > 0 {
        eprintln!(
            "  {}",
            s.muted(format!("Skipping {dup_selected} duplicate(s)"))
        );
        selected.retain(|c| !c.is_duplicate);
    }

    if selected.is_empty() {
        eprintln!("No non-duplicate cards selected. Exiting.");
        return Ok(());
    }

    // 7. Quality check
    let quality_result = if let Some(ref client) = client {
        perform_quality_check(
            selected,
            frontmatter.quality_check.as_ref(),
            client,
            &runtime.model,
            runtime.temperature,
            runtime.max_tokens,
            runtime.retries,
            Some(&logger),
        )?
    } else {
        super::quality::QualityCheckResult {
            final_cards: selected,
            cost: 0.0,
        }
    };

    if quality_result.final_cards.is_empty() {
        eprintln!("\nNo cards remaining after quality check. Exiting.");
        return Ok(());
    }

    // Report total cost
    let total_cost = generation_cost + quality_result.cost;
    if total_cost > 0.0 {
        eprintln!(
            "\n  {}  {}",
            s.muted("Total cost"),
            s.accent(pricing::format_cost(total_cost))
        );
    }

    // 8. Export or import
    if let Some(ref output_path) = args.output {
        export_cards(&quality_result.final_cards, output_path)?;
    } else {
        let result = import_cards_to_anki(&quality_result.final_cards, &frontmatter, &anki)?;
        report_import_result(&result, &frontmatter.deck);

        if result.failures > 0 {
            anyhow::bail!(
                "Import failed: {} card(s) could not be added. Check your Anki collection and try again.",
                result.failures
            );
        }
    }

    Ok(())
}

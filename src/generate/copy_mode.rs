use anyhow::Result;

use crate::anki::client::anki_client;
use crate::cli::GenerateArgs;
use crate::llm::parse_json::try_parse_json_array;
use crate::style::Style;

use super::anki_import::{finalize_tts, import_cards_to_anki, report_import_result};
use super::cards::ValidatedCard;
use super::exporter::export_cards;
use super::manual::get_llm_response_manually;
use super::processor::CardCandidate;
use super::selector::{display_cards, select_cards_legacy};
use super::session::load_prompt;
use super::validate::validate_anki_assets;

/// Manual copy-paste mode — loads prompt and Anki config but skips LLM client setup.
pub(super) fn run_copy_mode(
    args: &GenerateArgs,
    term: &str,
    s: &Style,
    on_log: &(dyn Fn(&str) + Send + Sync),
) -> Result<()> {
    let loaded = load_prompt(args)?;
    let frontmatter = &loaded.frontmatter;

    let has_processing = frontmatter
        .processing
        .as_ref()
        .map(|p| !p.pre_select.is_empty() || !p.post_select.is_empty())
        .unwrap_or(false);
    if has_processing {
        anyhow::bail!("processing is not supported in --copy mode");
    }

    eprintln!("  {}  {}", s.muted("Deck     "), s.cyan(&frontmatter.deck));
    eprintln!(
        "  {}  {}",
        s.muted("Note type"),
        s.cyan(&frontmatter.note_type)
    );

    let anki = anki_client();
    let validation = validate_anki_assets(&anki, frontmatter)?;
    eprintln!(
        "  {}  {}",
        s.muted("Fields   "),
        s.muted(validation.note_type_fields.join(", "))
    );

    let field_map_keys: Vec<String> = frontmatter.field_map.keys().cloned().collect();

    let mut row = crate::data::Row::new();
    row.insert("term".into(), serde_json::Value::String(term.to_string()));
    row.insert(
        "count".into(),
        serde_json::Value::String(args.count.to_string()),
    );
    let filled = crate::template::fill_template(&loaded.body, &row)?;
    let raw = get_llm_response_manually(&filled)?;

    let parsed_arr = try_parse_json_array(&raw)
        .ok_or_else(|| anyhow::anyhow!("Response is not a valid JSON array"))?;

    let mut skipped = 0;
    let candidates: Vec<CardCandidate> = parsed_arr
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

    // Sanitize and validate
    let sanitized_pairs: Vec<_> = candidates
        .into_iter()
        .map(|c| {
            let s = super::sanitize::sanitize_fields(&c.fields);
            (c, s)
        })
        .collect();

    let first_field_name = &validation.note_type_fields[0];
    let validated =
        super::cards::validate_cards(sanitized_pairs, frontmatter, first_field_name, &anki, "")?;

    let dup_count = validated.iter().filter(|c| c.is_duplicate).count();
    if dup_count > 0 {
        eprintln!(
            "  {}",
            s.muted(format!("{dup_count} duplicate(s) already in Anki"))
        );
    }

    if args.dry_run {
        display_cards(&validated);
        return Ok(());
    }

    if validated.is_empty() {
        eprintln!("No cards to select from.");
        return Ok(());
    }

    let selected_indices = select_cards_legacy(&validated)?;
    let mut selected: Vec<ValidatedCard> = selected_indices
        .iter()
        .filter_map(|&i| validated.get(i).cloned())
        .collect();

    if selected.is_empty() {
        eprintln!("\nNo cards selected. Exiting.");
        return Ok(());
    }

    // Export or import
    if let Some(ref output_path) = args.output {
        export_cards(&selected, output_path, on_log)?;
    } else {
        let tts_bundle = if let Some(ref spec) = frontmatter.tts {
            // TTS credentials come from env/config, not from generate's
            // `--api-key`/`--api-base-url` which target the LLM endpoint.
            Some(crate::tts::service::build_bundle(
                spec,
                anki_client(),
                crate::tts::service::TtsBundleOptions { azure_region: None },
            )?)
        } else {
            None
        };
        if let Some(b) = tts_bundle.as_ref() {
            let finalizer = crate::generate::anki_import::TtsFinalize {
                service: &b.service,
                media: b.media.as_ref(),
            };
            finalize_tts(&mut selected, frontmatter, finalizer, on_log)?;
        }
        let result = import_cards_to_anki(&mut selected, frontmatter, &anki, on_log)?;
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

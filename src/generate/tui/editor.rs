use ratatui::DefaultTerminal;

use super::state::{App, AppMode, Toast};

use crate::anki::client::anki_client;

/// Suspend the TUI, open the focused card in $EDITOR, and apply edits.
pub(super) fn edit_card_in_editor(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    card_index: usize,
) {
    let AppMode::Selecting(ref state) = app.mode else {
        return;
    };
    let Some(card) = state.cards.get(card_index) else {
        return;
    };
    let Some(ref info) = app.session_info else {
        return;
    };

    // Build ordered YAML from raw_anki_fields (Anki field names → raw markdown).
    // We use a Vec of (key, value) to preserve field order via serde_yaml.
    let fields_for_edit: indexmap::IndexMap<String, String> = card.raw_anki_fields.clone();
    let yaml = match serde_yaml::to_string(&fields_for_edit) {
        Ok(y) => y,
        Err(e) => {
            app.toast = Some(Toast {
                message: format!("Failed to serialize: {e}"),
                tick: app.tick,
            });
            return;
        }
    };

    // Write to temp file
    let tmp_dir = std::env::temp_dir();
    let tmp_path = tmp_dir.join("anki-llm-edit.yaml");
    if std::fs::write(&tmp_path, &yaml).is_err() {
        app.toast = Some(Toast {
            message: "Failed to write temp file".into(),
            tick: app.tick,
        });
        return;
    }

    // Determine editor
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "vi".to_string());

    // Suspend TUI
    crossterm::execute!(std::io::stdout(), crossterm::event::DisableBracketedPaste).ok();
    ratatui::restore();

    // Spawn editor
    let status = std::process::Command::new(&editor).arg(&tmp_path).status();

    // Resume TUI
    *terminal = ratatui::init();
    crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste).ok();

    let ok = match status {
        Ok(s) if s.success() => true,
        Ok(_) => {
            app.toast = Some(Toast {
                message: "Editor exited with error".into(),
                tick: app.tick,
            });
            false
        }
        Err(e) => {
            app.toast = Some(Toast {
                message: format!("Failed to launch {editor}: {e}"),
                tick: app.tick,
            });
            false
        }
    };

    if !ok {
        let _ = std::fs::remove_file(&tmp_path);
        return;
    }

    // Read edited content
    let edited_yaml = match std::fs::read_to_string(&tmp_path) {
        Ok(s) => s,
        Err(e) => {
            app.toast = Some(Toast {
                message: format!("Failed to read edited file: {e}"),
                tick: app.tick,
            });
            return;
        }
    };
    let _ = std::fs::remove_file(&tmp_path);

    // Parse edited YAML (Anki field names → raw markdown)
    let edited_anki_fields: indexmap::IndexMap<String, String> =
        match parse_edited_anki_fields(&edited_yaml) {
            Ok(m) => m,
            Err(e) => {
                app.toast = Some(Toast {
                    message: format!("YAML parse error: {e}"),
                    tick: app.tick,
                });
                return;
            }
        };

    // Build reverse map: Anki name → LLM key
    let reverse_map: std::collections::HashMap<&str, &str> = info
        .field_map
        .iter()
        .map(|(llm_key, anki_name)| (anki_name.as_str(), llm_key.as_str()))
        .collect();

    // Rebuild fields (LLM keys → sanitized HTML) and raw_anki_fields
    let mut new_fields: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut new_raw_anki_fields: indexmap::IndexMap<String, String> = indexmap::IndexMap::new();
    let mut new_anki_fields: indexmap::IndexMap<String, String> = indexmap::IndexMap::new();

    for (anki_name, raw_value) in &edited_anki_fields {
        new_raw_anki_fields.insert(anki_name.clone(), raw_value.clone());
        let sanitized = crate::generate::sanitize::sanitize_html(raw_value);
        new_anki_fields.insert(anki_name.clone(), sanitized.clone());
        if let Some(&llm_key) = reverse_map.get(anki_name.as_str()) {
            new_fields.insert(llm_key.to_string(), sanitized);
        }
    }

    // Re-check duplicate status against the authoritative first-field
    // name from `SessionInfo` — `new_anki_fields` preserves whatever
    // order the user wrote in `$EDITOR`, so trusting its insertion
    // order would query Anki against the wrong field whenever the user
    // rearranged the YAML. Refresh the full duplicate metadata shape
    // (note id + fields) via the shared helper so the selection
    // screen's diff panel renders against up-to-date data rather than
    // the pre-edit (or stale) existing-note fields.
    let first_field_value = new_anki_fields
        .get(&info.first_field_name)
        .cloned()
        .unwrap_or_default();
    let (new_dup_note_id, new_duplicate_fields) = {
        let anki = anki_client();
        crate::generate::cards::lookup_duplicate_metadata(
            &anki,
            &first_field_value,
            &info.first_field_name,
            &info.note_type,
            &info.deck,
        )
        .unwrap_or((None, None))
    };
    let is_duplicate = new_dup_note_id.is_some();

    // Apply edits to the card. Mint a new `card_id` so any stale TTS
    // preview state (cached `Ready` path pointing at pre-edit audio,
    // or an in-flight `Synthesizing` reply) is invalidated by id
    // mismatch. Transfer selection/regen-flight membership from the
    // old id to the new one.
    let AppMode::Selecting(ref mut state) = app.mode else {
        return;
    };
    if let Some(card) = state.cards.get_mut(card_index) {
        let old_id = card.card_id;
        let new_id = crate::generate::cards::next_card_id();
        card.card_id = new_id;
        card.fields = new_fields;
        card.anki_fields = new_anki_fields;
        card.raw_anki_fields = new_raw_anki_fields;
        card.is_duplicate = is_duplicate;
        card.duplicate_note_id = new_dup_note_id;
        card.duplicate_fields = new_duplicate_fields;
        card.flags.clear(); // clear stale flags after manual edit

        if state.selected.remove(&old_id) {
            state.selected.insert(new_id);
        }
        // Editing semantically *cancels* an in-flight regeneration:
        // the worker is generating against the pre-edit text, so its
        // reply is no longer relevant. Clear the spinner now; the
        // late reply will be tagged with the old id and dropped on
        // arrival by `ReplaceCard`'s `iter_mut().find` lookup.
        if state.regen_in_flight == Some(old_id) {
            state.regen_in_flight = None;
        }
        state.tts_states.remove(&old_id);

        app.toast = Some(Toast {
            message: "Card updated".into(),
            tick: app.tick,
        });
    }
}

/// Deserialize the user's edited YAML back into an Anki-field-name
/// keyed map. Extracted so the post-`$EDITOR` parse + first-field
/// lookup is unit-testable without spawning an editor.
pub(super) fn parse_edited_anki_fields(
    yaml: &str,
) -> Result<indexmap::IndexMap<String, String>, serde_yaml::Error> {
    serde_yaml::from_str(yaml)
}

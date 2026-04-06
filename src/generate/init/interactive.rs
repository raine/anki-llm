use indexmap::IndexMap;
use inquire::Select;

use crate::anki::client::AnkiClient;
use crate::generate::init::util::{resolve_duplicate_keys, suggest_key_for_field};

pub type FieldMap = IndexMap<String, String>;

/// Select a deck interactively.
pub fn select_deck(anki: &AnkiClient) -> anyhow::Result<String> {
    let decks = anki.deck_names()?;
    if decks.is_empty() {
        anyhow::bail!("No decks found in Anki collection");
    }

    if decks.len() == 1 {
        return Ok(decks.into_iter().next().unwrap());
    }

    Select::new("Select the target deck:", decks)
        .prompt()
        .map_err(map_inquire_err)
}

/// Select a note type interactively.
pub fn select_note_type(anki: &AnkiClient, deck: &str) -> anyhow::Result<String> {
    let note_types = anki.find_model_names_for_deck(deck)?;

    // Fallback to all models if deck has no notes
    let note_types = if note_types.is_empty() {
        anki.model_names()?
    } else {
        note_types
    };

    if note_types.is_empty() {
        anyhow::bail!("No note types found");
    }

    if note_types.len() == 1 {
        return Ok(note_types.into_iter().next().unwrap());
    }

    Select::new("Select the note type:", note_types)
        .prompt()
        .map_err(map_inquire_err)
}

/// Configure field mapping interactively.
pub fn configure_field_mapping(anki: &AnkiClient, note_type: &str) -> anyhow::Result<FieldMap> {
    let fields = anki.model_field_names(note_type)?;
    if fields.is_empty() {
        anyhow::bail!("Note type has no fields");
    }

    // Generate suggested keys and resolve duplicates
    let suggested_keys: Vec<String> = fields.iter().map(|f| suggest_key_for_field(f)).collect();
    let resolved_keys = resolve_duplicate_keys(suggested_keys);

    let mut field_map: FieldMap = IndexMap::new();
    for (field, key) in fields.iter().zip(resolved_keys.iter()) {
        field_map.insert(field.clone(), key.clone());
    }

    // Show proposed mapping
    println!("\nProposed field mapping:");
    println!("{}", "-".repeat(40));
    for (field, key) in &field_map {
        println!("  {field} -> {key}");
    }
    println!("{}", "-".repeat(40));

    let use_proposed = inquire::Confirm::new("Accept this mapping?")
        .with_default(true)
        .prompt()
        .map_err(map_inquire_err)?;

    if use_proposed {
        // Swap to json_key -> anki_field for Frontmatter convention
        return Ok(field_map.into_iter().map(|(a, k)| (k, a)).collect());
    }

    // Let user edit each field
    let re_valid_key = regex::Regex::new(r"^[a-zA-Z_][a-zA-Z0-9_]*$").unwrap();

    let mut custom_map: FieldMap = IndexMap::new();
    for field in fields {
        loop {
            let input = inquire::Text::new(&format!("Key for field '{}':", field))
                .with_default(&suggest_key_for_field(&field))
                .prompt()
                .map_err(map_inquire_err)?;

            let key = input.trim().to_string();
            if key.is_empty() {
                println!("Key cannot be empty.");
                continue;
            }
            if !re_valid_key.is_match(&key) {
                println!(
                    "Invalid key '{}': must start with letter/underscore and contain only letters, numbers, underscores.",
                    key
                );
                continue;
            }
            if custom_map.values().any(|used| used == &key) {
                println!(
                    "Key '{key}' is already used by another field. Please choose a unique key."
                );
                continue;
            }
            // Store as anki_field -> json_key for display consistency; swap before return
            custom_map.insert(field.clone(), key);
            break;
        }
    }

    // Swap to json_key -> anki_field for Frontmatter convention
    Ok(custom_map.into_iter().map(|(a, k)| (k, a)).collect())
}

/// Map inquire errors to a consistent anyhow error.
pub(super) fn map_inquire_err(e: inquire::InquireError) -> anyhow::Error {
    match e {
        inquire::InquireError::OperationCanceled | inquire::InquireError::OperationInterrupted => {
            anyhow::anyhow!("User cancelled")
        }
        other => anyhow::anyhow!(other),
    }
}

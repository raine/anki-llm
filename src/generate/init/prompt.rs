use indexmap::IndexMap;

use crate::anki::client::{AnkiClient, anki_quote};
use crate::generate::init::interactive::FieldMap;
use crate::generate::init::util::is_auto_generated_field;
use crate::generate::manual::get_llm_response_manually;
use crate::llm::client::LlmClient;
use crate::llm::pricing;

pub struct PromptDraft {
    pub body: String,
    pub final_field_map: FieldMap,
}

/// Generate a generic boilerplate prompt body.
pub fn generate_generic_prompt_body(field_keys: &[String]) -> String {
    let mut example_json = serde_json::Map::new();
    for key in field_keys {
        example_json.insert(key.clone(), format!("Example value for {key}").into());
    }

    let array_example = format!(
        "[\n{}\n]",
        serde_json::to_string_pretty(&example_json)
            .unwrap()
            .lines()
            .map(|l| format!("  {l}"))
            .collect::<Vec<_>>()
            .join("\n")
    );

    format!(
        r#"You are an expert assistant who creates {{count}} distinct Anki flashcards for a vocabulary term.
The term to create cards for is: **{{term}}**

IMPORTANT: Your output must be a single, valid JSON array of objects and nothing else.
Do not include any explanation, markdown formatting, or additional text.
Each object in the array should represent a unique flashcard.
All field values must be strings.
For fields that require formatting (like lists or emphasis), generate a single string containing well-formed HTML.

Follow the structure and HTML formatting shown in this example precisely:

```json
{array_example}
```

Return only a valid JSON array matching this structure. Ensure you generate {{count}} varied and high-quality cards.

Tips for creating high-quality cards:
- Provide clear, concise definitions or translations
- Include natural, contextual examples that highlight different nuances of the term
- Ensure each card offers a unique perspective, context, or usage example
- Use HTML tags like <b>, <i>, <ul>, <li> for formatting when helpful
- For language learning: include pronunciation guides if relevant
- Keep the content focused and easy to review"#,
    )
}

/// Build the meta-prompt for contextual prompt generation.
fn build_meta_prompt(
    deck_name: &str,
    sample_cards: &[serde_json::Value],
    field_keys: &[String],
) -> String {
    let field_keys_str = field_keys.join(", ");
    format!(
        r#"You are an expert prompt engineer creating a prompt template for another AI.
Your goal is to generate a helpful and flexible prompt body that instructs an AI to create multiple new Anki cards that match the general style of the provided examples.

**IMPORTANT CONTEXT:**
- The user's deck is named "{deck_name}".
- You are working with a very small sample of existing cards.
- Your task is to infer the *likely principles and general style*, not to codify every detail as a strict rule. Prioritize patterns that are consistent across multiple examples and ignore coincidences.

**EXISTING CARD EXAMPLES:**
```json
{}
```

**YOUR TASK:**

**Step 1: Gentle Analysis**
Analyze the examples to understand the deck's high-level principles:

1. **Purpose & Style**: What is the likely subject matter and learning goal (e.g., conversational Japanese, medical terminology)?
2. **Content Principles**: What kind of information is typically included in fields like explanations or notes? Look for recurring themes (e.g., formal vs. informal usage, common mistakes, collocations). Distinguish between what seems essential versus what is helpful but optional.
3. **Formatting Conventions**: How is HTML used for emphasis and structure?
   - What is the *purpose* of tags like `<b>` or `<ul>`?
   - For linguistic formatting (like Japanese furigana `漢字[かんじ]`), identify the general pattern but **avoid creating overly strict spacing rules** from this small sample. Focus on high-confidence patterns only.

**Step 2: Generate a Flexible Prompt Body for MULTIPLE Cards**
Using your analysis, generate a prompt body that guides the AI to create a batch of cards that *fit the spirit* of the examples, while allowing for natural variation.

1. **Persona & Goal**: Start with a concise instruction for the AI, mentioning the deck's purpose and that it should generate **{{count}} distinct cards**. This placeholder is critical. For example: "You are an expert assistant who creates {{count}} distinct Anki flashcards for...".

2. **Term Placeholder**: Include a natural sentence that introduces the term/phrase using the **{{term}}** placeholder. For example: "The term to create cards for is: **{{term}}**".

3. **One-Shot Example (Array Format)**: Provide a single, plausible, **NEW** example inside a JSON array code block. This example should be a good demonstration of the deck's style. The JSON keys must be exactly: {field_keys_str}. The output format must be `[ {{ ...card_object... }} ]`.

4. **Boilerplate**: Include the standard instruction: "IMPORTANT: Your output must be a single, valid JSON array of objects and nothing else. Do not include any explanation, markdown formatting, or additional text. All field values must be strings."

5. **Stylistic & Diversity Guidelines (Not Strict Rules)**:
   - Create sections with headings like "Formatting Guidelines" and "Content Guidelines".
   - Phrase instructions as recommendations, not commands. Use words like "Generally," "Typically," "Aim to," "Consider including."
   - **Add instructions to encourage diversity**: "Ensure each card offers a unique perspective on the term, such as a different definition, context, or example sentence."
   - **Good Example**: "Typically, use `<b>` tags to highlight the main term within example sentences."
   - **Bad Example**: "You must always bold the second word of every sentence."
   - If a field was often empty in the samples, suggest its purpose rather than mandating it be empty. Example: "The 'notes' field is optional but can be used for extra cultural context."
   - For complex formatting like furigana, provide a single good example and a brief, high-level description of the pattern. Avoid detailed CORRECT/INCORRECT lists unless a pattern is exceptionally clear and consistent across all samples.

**OUTPUT FORMAT:**
Return ONLY the raw text for the prompt body. Do NOT include frontmatter or explanations about your process."#,
        serde_json::to_string_pretty(sample_cards).unwrap()
    )
}

/// Maximum notes to fetch from AnkiConnect when sampling a deck.
const MAX_SAMPLE_NOTES: usize = 200;

/// Create prompt content, either contextual or generic fallback.
#[allow(clippy::too_many_arguments)]
pub fn create_prompt_content(
    anki: &AnkiClient,
    deck: &str,
    note_type: &str,
    initial_field_map: &FieldMap,
    llm_client: &LlmClient,
    model: &str,
    temperature: Option<f64>,
    copy: bool,
) -> Result<PromptDraft, anyhow::Error> {
    // Find notes for the specific deck + note type
    let query = format!("deck:{} note:{}", anki_quote(deck), anki_quote(note_type));
    let mut note_ids = anki
        .find_notes(&query)
        .map_err(|e| anyhow::anyhow!("Failed to find notes in deck: {e}"))?;

    if note_ids.is_empty() {
        return Err(anyhow::anyhow!("No cards found in deck to analyze."));
    }

    eprintln!(
        "  Analyzing {} card(s) to find best examples...",
        note_ids.len()
    );

    // Limit how many notes we fetch to avoid overwhelming AnkiConnect
    note_ids.truncate(MAX_SAMPLE_NOTES);

    // Fetch sampled notes to find best examples
    let all_notes = anki
        .notes_info(&note_ids)
        .map_err(|e| anyhow::anyhow!("Failed to fetch note info: {e}"))?;

    // Score notes by non-empty, non-auto fields
    let mut scored: Vec<(usize, _)> = all_notes
        .iter()
        .enumerate()
        .map(|(idx, note)| {
            let score = initial_field_map
                .values()
                .filter(|anki_field| {
                    note.fields
                        .get(*anki_field)
                        .map(|f| !f.value.trim().is_empty() && !is_auto_generated_field(&f.value))
                        .unwrap_or(false)
                })
                .count();
            (idx, score)
        })
        .collect();

    scored.sort_by(|a, b| b.1.cmp(&a.1));
    let top_indices: Vec<usize> = scored.into_iter().take(5).map(|(idx, _)| idx).collect();

    eprintln!(
        "  Selected {} card(s) with most populated fields...",
        top_indices.len()
    );

    if top_indices.is_empty() {
        return Err(anyhow::anyhow!(
            "Found cards, but none have usable content for analysis."
        ));
    }

    // Filter out auto-generated fields
    let first_note = &all_notes[top_indices[0]];
    let mut final_field_map: FieldMap = IndexMap::new();
    let mut skipped_fields: Vec<&str> = Vec::new();

    for (json_key, anki_field) in initial_field_map {
        let sample_value = first_note
            .fields
            .get(anki_field)
            .map(|f| f.value.as_str())
            .unwrap_or("");

        if is_auto_generated_field(sample_value) {
            skipped_fields.push(anki_field);
        } else {
            final_field_map.insert(json_key.clone(), anki_field.clone());
        }
    }

    if !skipped_fields.is_empty() {
        eprintln!(
            "  Skipping auto-generated field(s): {}",
            skipped_fields.join(", ")
        );
    }

    // Build sample cards for meta-prompt
    let sample_cards: Vec<serde_json::Value> = top_indices
        .iter()
        .map(|&idx| {
            let note = &all_notes[idx];
            let mut card = serde_json::Map::new();
            for (json_key, anki_field) in &final_field_map {
                card.insert(
                    json_key.clone(),
                    serde_json::Value::String(
                        note.fields
                            .get(anki_field)
                            .map(|f| f.value.clone())
                            .unwrap_or_default(),
                    ),
                );
            }
            serde_json::Value::Object(card)
        })
        .collect();

    // Build meta-prompt
    let meta_prompt = build_meta_prompt(
        deck,
        &sample_cards,
        &final_field_map.keys().cloned().collect::<Vec<_>>(),
    );

    if copy {
        // Manual mode
        let body = get_llm_response_manually(&meta_prompt)?;
        eprintln!("\nSmart prompt generated successfully!");
        return Ok(PromptDraft {
            body: body.trim().to_string(),
            final_field_map,
        });
    }

    // Call LLM
    let spinner = crate::spinner::llm_spinner(format!("Generating smart prompt using {model}..."));
    match llm_client.chat_completion(model, &meta_prompt, temperature, None) {
        Ok(response) => {
            spinner.finish_and_clear();
            eprintln!("Smart prompt generated successfully!");
            if let Some(usage) = &response.usage {
                let cost =
                    pricing::calculate_cost(model, usage.prompt_tokens, usage.completion_tokens);
                eprintln!(
                    "  Cost: {} ({} input + {} output tokens)",
                    pricing::format_cost(cost),
                    usage.prompt_tokens,
                    usage.completion_tokens,
                );
            }
            Ok(PromptDraft {
                body: response.content.trim().to_string(),
                final_field_map,
            })
        }
        Err(e) => {
            spinner.finish_and_clear();
            eprintln!("Could not generate smart prompt. Falling back to generic template.");
            eprintln!("  Reason: {}\n", e);
            Ok(PromptDraft {
                body: generate_generic_prompt_body(
                    &final_field_map.keys().cloned().collect::<Vec<_>>(),
                ),
                final_field_map,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_generic_prompt_body_test() {
        let keys = vec!["front".to_string(), "back".to_string()];
        let body = generate_generic_prompt_body(&keys);
        assert!(body.contains("{term}"));
        assert!(body.contains("{count}"));
        assert!(body.contains("front"));
        assert!(body.contains("back"));
    }
}

import { z } from 'zod';
import { ankiRequest } from '../anki-connect.js';
import { getFieldNamesForModel } from '../anki-schema.js';

// Assuming a type for frontmatter exists
interface PromptFrontmatter {
  deck: string;
  noteType: string;
  fieldMap: Record<string, string>;
}

export interface AnkiValidationResult {
  noteTypeFields: string[];
}

/**
 * Validates deck, note type, and field mappings against the user's Anki collection.
 * Throws a descriptive error if validation fails.
 * @returns The list of fields for the validated note type.
 */
export async function validateAnkiAssets(
  frontmatter: PromptFrontmatter,
): Promise<AnkiValidationResult> {
  // Validate deck
  const deckNames = await ankiRequest('deckNames', z.array(z.string()), {});
  if (!deckNames.includes(frontmatter.deck)) {
    throw new Error(
      `Deck "${frontmatter.deck}" does not exist in Anki. Available decks: ${deckNames.join(', ')}`,
    );
  }

  // Validate note type
  let noteTypeFields: string[];
  try {
    noteTypeFields = await getFieldNamesForModel(frontmatter.noteType);
  } catch {
    const modelNames = await ankiRequest('modelNames', z.array(z.string()), {});
    throw new Error(
      `Note type "${frontmatter.noteType}" does not exist. Available note types: ${modelNames.join(', ')}`,
    );
  }

  // Validate fieldMap
  const mappedFields = Object.values(frontmatter.fieldMap);
  const invalidFields = mappedFields.filter((f) => !noteTypeFields.includes(f));
  if (invalidFields.length > 0) {
    throw new Error(
      `The following fields from your fieldMap do not exist in note type "${frontmatter.noteType}": ${invalidFields.join(', ')}`,
    );
  }

  return { noteTypeFields };
}

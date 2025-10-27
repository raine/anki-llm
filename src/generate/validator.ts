import { z } from 'zod';
import { ankiRequest } from '../anki-connect.js';
import type { CardCandidate } from './processor.js';
import type { Frontmatter } from '../utils/parse-frontmatter.js';

export interface ValidatedCard extends CardCandidate {
  isDuplicate: boolean;
  ankiFields: Record<string, string>; // Mapped to actual Anki field names
}

/**
 * Maps card fields from LLM JSON keys to actual Anki field names.
 *
 * @param card - Card with fields using LLM JSON keys
 * @param fieldMap - Mapping from LLM keys to Anki field names
 * @returns Object with Anki field names
 */
export function mapFieldsToAnki(
  card: CardCandidate,
  fieldMap: Record<string, string>,
): Record<string, string> {
  const ankiFields: Record<string, string> = {};

  for (const [llmKey, ankiFieldName] of Object.entries(fieldMap)) {
    const value = card.fields[llmKey];
    if (value === undefined) {
      throw new Error(
        `Missing field "${llmKey}" in card. Expected fields: ${Object.keys(fieldMap).join(', ')}`,
      );
    }
    ankiFields[ankiFieldName] = value;
  }

  return ankiFields;
}

/**
 * Checks if a card already exists in Anki by querying the first field.
 *
 * Anki's uniqueness constraint is: Note Type + First Field value.
 * This function queries for existing notes with matching first field content.
 *
 * @param firstFieldValue - Value of the first field (the unique identifier)
 * @param noteType - The Anki note type name
 * @param deck - The target deck name
 * @returns True if a duplicate exists, false otherwise
 */
async function checkDuplicate(
  firstFieldValue: string,
  noteType: string,
  deck: string,
): Promise<boolean> {
  try {
    // Query for notes with this exact first field value in this deck and note type
    // AnkiConnect's findNotes uses a query string
    const query = `"note:${noteType}" "deck:${deck}" "${firstFieldValue}"`;

    const noteIds = await ankiRequest('findNotes', z.array(z.number()), {
      query,
    });

    return noteIds.length > 0;
  } catch (error) {
    // If the query fails (e.g., invalid note type), log a warning but don't fail
    console.warn(
      `Warning: Could not check for duplicates: ${error instanceof Error ? error.message : String(error)}`,
    );
    return false;
  }
}

/**
 * Validates and enriches card candidates by:
 * 1. Mapping fields to Anki field names
 * 2. Checking for duplicates
 *
 * @param cards - Array of card candidates from LLM
 * @param frontmatter - Prompt file frontmatter with deck, noteType, fieldMap
 * @param firstFieldName - Name of the first field (for duplicate detection)
 * @returns Array of validated cards with duplicate status and mapped fields
 */
export async function validateCards(
  cards: CardCandidate[],
  frontmatter: Frontmatter,
  firstFieldName: string,
): Promise<ValidatedCard[]> {
  const validationPromises = cards.map(async (card) => {
    // Map fields from LLM keys to Anki field names
    const ankiFields = mapFieldsToAnki(card, frontmatter.fieldMap);

    // Get the first field value for duplicate detection
    const firstFieldValue = ankiFields[firstFieldName];

    if (!firstFieldValue) {
      console.warn(
        `Warning: Card is missing first field "${firstFieldName}", skipping duplicate check`,
      );
      return {
        ...card,
        isDuplicate: false,
        ankiFields,
      };
    }

    // Check if this card already exists
    const isDuplicate = await checkDuplicate(
      firstFieldValue,
      frontmatter.noteType,
      frontmatter.deck,
    );

    return {
      ...card,
      isDuplicate,
      ankiFields,
    };
  });

  return Promise.all(validationPromises);
}

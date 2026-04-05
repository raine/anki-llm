import { z } from 'zod';
import { ankiRequest, NoteInfo } from './anki-connect.js';

/**
 * Fetches the field names for a given Anki model (note type).
 * @param modelName The name of the model.
 * @returns A promise that resolves to an array of field names.
 */
export async function getFieldNamesForModel(
  modelName: string,
): Promise<string[]> {
  return await ankiRequest('modelFieldNames', z.array(z.string()), {
    modelName,
  });
}

/**
 * Finds the model name used by notes in a given deck.
 * This is useful for automatically determining the target model.
 * @param deckName The name of the deck.
 * @returns A promise that resolves to the model name, or null if the deck is empty.
 */
export async function findModelNameForDeck(
  deckName: string,
): Promise<string | null> {
  const noteIds = await ankiRequest('findNotes', z.array(z.number()), {
    query: `deck:"${deckName}"`,
  });

  if (noteIds.length === 0) {
    return null;
  }

  // Check the first note to find the model name,
  // assuming the deck is homogenous.
  const notesInfo = await ankiRequest('notesInfo', z.array(NoteInfo), {
    notes: [noteIds[0]],
  });

  return notesInfo[0]?.modelName ?? null;
}

/**
 * Finds all unique note types (models) used in a given deck.
 * @param deckName The name of the deck.
 * @returns A promise that resolves to an array of unique model names, or empty array if deck is empty.
 */
export async function findModelNamesForDeck(
  deckName: string,
): Promise<string[]> {
  const noteIds = await ankiRequest('findNotes', z.array(z.number()), {
    query: `deck:"${deckName}"`,
  });

  if (noteIds.length === 0) {
    return [];
  }

  // Fetch info for all notes to get their model names
  const notesInfo = await ankiRequest('notesInfo', z.array(NoteInfo), {
    notes: noteIds,
  });

  // Extract unique model names
  const modelNames = new Set<string>();
  for (const note of notesInfo) {
    if (note.modelName) {
      modelNames.add(note.modelName);
    }
  }

  return Array.from(modelNames).sort();
}

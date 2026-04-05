import { select, confirm, input } from '@inquirer/prompts';
import chalk from 'chalk';
import { z } from 'zod';
import { ankiRequest } from '../anki-connect.js';
import {
  getFieldNamesForModel,
  findModelNamesForDeck,
} from '../anki-schema.js';
import { suggestKeyForField, resolveDuplicateKeys } from './util.js';

export async function selectDeck(): Promise<string> {
  console.log(chalk.cyan('\n📚 Fetching your Anki decks...'));
  const deckNames = await ankiRequest('deckNames', z.array(z.string()), {});

  if (deckNames.length === 0) {
    throw new Error('No decks found in Anki. Create a deck first.');
  }

  const selectedDeck = await select({
    message: 'Select the target deck:',
    choices: deckNames.map((name) => ({ value: name })),
    pageSize: 15,
    theme: {
      keybindings: ['vim'],
    },
  });

  console.log(chalk.green(`✓ Selected deck: ${selectedDeck}`));
  return selectedDeck;
}

export async function selectNoteType(deckName: string): Promise<string> {
  console.log(chalk.cyan('\n📋 Fetching note types used in this deck...'));
  let modelNameChoices = await findModelNamesForDeck(deckName);

  if (modelNameChoices.length === 0) {
    console.log(
      chalk.yellow(
        `⚠️  Deck "${deckName}" has no notes. Showing all available note types instead.\n`,
      ),
    );

    // Fallback to all note types if deck is empty
    modelNameChoices = await ankiRequest('modelNames', z.array(z.string()), {});

    if (modelNameChoices.length === 0) {
      throw new Error('No note types found in your Anki collection.');
    }
  }

  if (modelNameChoices.length === 1) {
    const selectedNoteType = modelNameChoices[0];
    console.log(
      chalk.green(
        `✓ Auto-selected the only available note type: ${selectedNoteType}`,
      ),
    );
    return selectedNoteType;
  }

  const selectedNoteType = await select({
    message: 'Select the note type:',
    choices: modelNameChoices.map((name) => ({ value: name })),
    pageSize: 15,
    theme: {
      keybindings: ['vim'],
    },
  });

  console.log(chalk.green(`✓ Selected note type: ${selectedNoteType}`));
  return selectedNoteType;
}

export async function configureFieldMapping(
  noteTypeName: string,
): Promise<Record<string, string>> {
  console.log(chalk.cyan('\n🔍 Fetching fields...'));
  const fieldNames = await getFieldNamesForModel(noteTypeName);

  console.log(
    chalk.gray(`Found ${fieldNames.length} field(s): ${fieldNames.join(', ')}`),
  );

  // Step 4: Create field mapping with auto-suggestion and review
  console.log(
    chalk.cyan('\n🗺️  Creating field mapping (LLM JSON keys → Anki fields)...'),
  );

  // Auto-suggest keys for all fields
  const suggestedKeys = fieldNames.map(suggestKeyForField);

  // Detect and resolve duplicate keys
  const resolvedKeys = resolveDuplicateKeys(suggestedKeys);

  // Build initial fieldMap
  let fieldMap: Record<string, string> = {};
  for (let i = 0; i < fieldNames.length; i++) {
    fieldMap[resolvedKeys[i]] = fieldNames[i];
  }

  // Display proposed mapping
  console.log(chalk.gray('Proposed mapping:'));
  for (const [key, value] of Object.entries(fieldMap)) {
    console.log(chalk.gray(`  ${key} → ${value}`));
  }

  // Ask user to accept or customize
  const acceptMapping = await confirm({
    message: 'Accept this mapping?',
    default: true,
  });

  if (!acceptMapping) {
    console.log(
      chalk.gray(
        '\nCustomize the mapping. Press Enter to keep the suggested key.\n',
      ),
    );

    // Clear and rebuild fieldMap with user input
    const customFieldMap: Record<string, string> = {};

    for (const fieldName of fieldNames) {
      const currentKey = Object.keys(fieldMap).find(
        (k) => fieldMap[k] === fieldName,
      )!;

      const jsonKey = await input({
        message: `JSON key for "${fieldName}":`,
        default: currentKey,
        validate: (input: string) => {
          if (!input.trim()) {
            return 'JSON key cannot be empty';
          }
          if (!/^[a-zA-Z_][a-zA-Z0-9_]*$/.test(input)) {
            return 'Invalid key. Use letters, numbers, and underscores only.';
          }
          return true;
        },
      });
      customFieldMap[jsonKey.trim()] = fieldName;
    }

    // Replace fieldMap with custom mapping
    fieldMap = customFieldMap;
  }

  console.log(chalk.green('\n✓ Field mapping complete'));
  return fieldMap;
}

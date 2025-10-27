import { z } from 'zod';
import chalk from 'chalk';
import { ankiRequest } from '../anki-connect.js';
import type { ValidatedCard } from './validator.js';

// Assuming a type for frontmatter
interface PromptFrontmatter {
  deck: string;
  noteType: string;
}

export interface ImportResult {
  successes: number;
  failures: number;
}

/**
 * Adds selected cards as notes to Anki.
 * @returns An object with the count of successful and failed imports.
 */
export async function importCardsToAnki(
  selectedCards: ValidatedCard[],
  frontmatter: PromptFrontmatter,
): Promise<ImportResult> {
  if (selectedCards.length === 0) {
    return { successes: 0, failures: 0 };
  }

  console.log(
    chalk.cyan(`\n📥 Adding ${selectedCards.length} card(s) to Anki...`),
  );

  const notesToAdd = selectedCards.map((card) => ({
    deckName: frontmatter.deck,
    modelName: frontmatter.noteType,
    fields: card.ankiFields,
    tags: ['anki-llm-generate'],
  }));

  const addResult = await ankiRequest(
    'addNotes',
    z.array(z.number().nullable()),
    { notes: notesToAdd },
  );

  const successes = addResult.filter((r) => r !== null).length;
  const failures = addResult.length - successes;

  return { successes, failures };
}

/**
 * Logs the final outcome of the import process to the console.
 */
export function reportImportResult(
  result: ImportResult,
  deckName: string,
): void {
  if (result.failures > 0) {
    console.log(
      chalk.yellow(
        `\n⚠️  Added ${result.successes} card(s), ${result.failures} failed.`,
      ),
    );
    console.log(
      chalk.gray(
        'Some cards may have been duplicates or had invalid field values.',
      ),
    );
  } else if (result.successes > 0) {
    console.log(
      chalk.green(
        `\n✓ Successfully added ${result.successes} new note(s) to "${deckName}"`,
      ),
    );
  } else {
    // This case can happen if no cards were selected
    console.log(chalk.yellow('\n⚠️  No cards were added to Anki.'));
  }
}

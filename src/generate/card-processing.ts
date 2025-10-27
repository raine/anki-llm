import chalk from 'chalk';
import { sanitizeFields } from '../utils/sanitize-html.js';
import { validateCards, type ValidatedCard } from './validator.js';
import { selectCards, displayCards } from './selector.js';
import type { CardCandidate } from './processor.js';

// Assuming a type for frontmatter
interface PromptFrontmatter {
  deck: string;
  noteType: string;
  fieldMap: Record<string, string>;
}

/**
 * Sanitizes, validates, and allows user selection of generated cards.
 * Handles dry-run logic internally.
 * @returns An array of cards selected by the user for import.
 */
export async function processAndSelectCards(
  generatedCards: CardCandidate[],
  frontmatter: PromptFrontmatter,
  noteTypeFields: string[],
  isDryRun: boolean,
): Promise<ValidatedCard[]> {
  // 1. Sanitize
  const sanitizedCards: CardCandidate[] = generatedCards.map((card) => ({
    ...card,
    fields: sanitizeFields(card.fields),
  }));

  // 2. Validate (check for duplicates)
  console.log(chalk.cyan('\n🔍 Checking for duplicates...'));
  const firstFieldName = noteTypeFields[0];
  const validatedCards = await validateCards(
    sanitizedCards,
    frontmatter,
    firstFieldName,
  );

  const duplicateCount = validatedCards.filter((c) => c.isDuplicate).length;
  if (duplicateCount > 0) {
    console.log(
      chalk.yellow(
        `⚠️  Found ${duplicateCount} duplicate(s) (already in Anki)`,
      ),
    );
  }

  // 3. Handle dry run or interactive selection
  if (isDryRun) {
    displayCards(validatedCards);
    return []; // Return empty array to signal no import
  }

  const selectedIndices = await selectCards(validatedCards);
  return selectedIndices.map((i) => validatedCards[i]);
}

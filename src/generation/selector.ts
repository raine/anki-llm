import inquirer from 'inquirer';
import chalk from 'chalk';
import type { ValidatedCard } from './validator.js';

/**
 * Formats a card for display in the selection UI.
 * Shows key fields in a readable format.
 *
 * @param card - The validated card to format
 * @param index - Card number (1-indexed)
 * @returns Formatted string for display
 */
function formatCardForDisplay(card: ValidatedCard, index: number): string {
  const lines: string[] = [];

  // Header with card number and duplicate warning
  const header = card.isDuplicate
    ? chalk.yellow(`Card ${index} (⚠️  Duplicate)`)
    : chalk.cyan(`Card ${index}`);
  lines.push(header);

  // Show first 3 fields (or all if fewer than 3)
  const fieldEntries = Object.entries(card.ankiFields);
  const fieldsToShow = fieldEntries.slice(0, 3);

  for (const [fieldName, value] of fieldsToShow) {
    // Truncate long values for display
    const displayValue =
      value.length > 100 ? `${value.substring(0, 100)}...` : value;
    // Remove HTML tags for cleaner display
    const cleanValue = displayValue.replace(/<[^>]*>/g, '');
    lines.push(`  ${chalk.gray(fieldName + ':')} ${cleanValue}`);
  }

  // Indicate if there are more fields
  if (fieldEntries.length > 3) {
    lines.push(
      chalk.gray(`  ... and ${fieldEntries.length - 3} more field(s)`),
    );
  }

  return lines.join('\n');
}

/**
 * Presents cards to the user in an interactive checklist.
 * Returns the indices of selected cards.
 *
 * @param cards - Array of validated cards to present
 * @returns Array of selected card indices (0-indexed)
 */
export async function selectCards(cards: ValidatedCard[]): Promise<number[]> {
  if (cards.length === 0) {
    throw new Error('No cards to select from');
  }

  // Check if we're in a TTY environment
  if (!process.stdout.isTTY) {
    throw new Error(
      'Interactive selection requires a TTY environment. ' +
        'Use --dry-run to preview cards without interaction.',
    );
  }

  console.log(
    chalk.cyan(
      `\n📋 Select cards to add to Anki (use Space to select, Enter to confirm):\n`,
    ),
  );

  // Create choices for inquirer
  const choices = cards.map((card, index) => ({
    name: formatCardForDisplay(card, index + 1),
    value: index,
    // Pre-select non-duplicates by default
    checked: !card.isDuplicate,
  }));

  try {
    const answers = (await inquirer.prompt([
      {
        type: 'checkbox',
        name: 'selectedCards',
        message: 'Choose cards to import:',
        choices,
        pageSize: 10, // Show 10 items at a time for easier navigation
        // Validate that at least one card is selected
        validate: (selected: unknown) => {
          const selectedArray = selected as number[];
          if (selectedArray.length === 0) {
            return 'Please select at least one card, or press Ctrl+C to cancel';
          }
          return true;
        },
      },
    ])) as { selectedCards: number[] };

    return answers.selectedCards;
  } catch (error) {
    // Handle user cancellation (Ctrl+C)
    if (error instanceof Error && error.message.includes('User force closed')) {
      console.log(chalk.yellow('\n\nSelection cancelled by user.'));
      process.exit(0);
    }
    throw error;
  }
}

/**
 * Displays cards in a formatted, human-readable list without interaction.
 * Used for --dry-run mode.
 *
 * @param cards - Array of validated cards to display
 */
export function displayCards(cards: ValidatedCard[]): void {
  if (cards.length === 0) {
    console.log(chalk.yellow('\nNo cards generated.'));
    return;
  }

  console.log(chalk.cyan(`\n📄 Generated ${cards.length} card(s):\n`));
  console.log(chalk.gray('─'.repeat(60)));

  for (let i = 0; i < cards.length; i++) {
    const card = cards[i];

    // Header
    const header = card.isDuplicate
      ? chalk.yellow(`\nCard ${i + 1} (⚠️  Duplicate - already exists in Anki)`)
      : chalk.cyan(`\nCard ${i + 1}`);
    console.log(header);

    // All fields
    for (const [fieldName, value] of Object.entries(card.ankiFields)) {
      console.log(chalk.gray(`\n${fieldName}:`));
      console.log(value);
    }

    console.log(chalk.gray('\n' + '─'.repeat(60)));
  }

  // Summary
  const duplicateCount = cards.filter((c) => c.isDuplicate).length;
  if (duplicateCount > 0) {
    console.log(
      chalk.yellow(
        `\n⚠️  ${duplicateCount} card(s) are duplicates (already exist in Anki)`,
      ),
    );
  }

  console.log(
    chalk.gray(
      '\nThis is a dry run. No cards were added to Anki.\n' +
        'Run without --dry-run to add cards interactively.',
    ),
  );
}

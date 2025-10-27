import inquirer from 'inquirer';
import chalk from 'chalk';
import type { ValidatedCard } from '../types.js';

const BOLD_START_MARK = '\u0000';
const BOLD_END_MARK = '\u0001';
const DISPLAY_CHAR_LIMIT = 100;

function stripHtmlPreserveBold(value: string): string {
  return value
    .replace(/<\s*b\s*>/gi, BOLD_START_MARK)
    .replace(/<\s*\/\s*b\s*>/gi, BOLD_END_MARK)
    .replace(/<[^>]*>/g, '');
}

function truncateWithMarkers(value: string, limit: number): string {
  let count = 0;
  let truncated = '';
  let boldActive = false;

  for (const char of value) {
    if (char === BOLD_START_MARK) {
      boldActive = true;
      truncated += char;
      continue;
    }

    if (char === BOLD_END_MARK) {
      boldActive = false;
      truncated += char;
      continue;
    }

    if (count >= limit) {
      truncated += '...';
      if (boldActive) {
        truncated += BOLD_END_MARK;
      }
      break;
    }

    truncated += char;
    count += 1;
  }

  return truncated;
}

function applyBoldMarkers(value: string): string {
  let result = '';
  let buffer = '';
  let bold = false;

  const flushBuffer = () => {
    if (buffer.length === 0) {
      return;
    }
    result += bold ? chalk.bold(buffer) : buffer;
    buffer = '';
  };

  for (const char of value) {
    if (char === BOLD_START_MARK) {
      flushBuffer();
      bold = true;
      continue;
    }

    if (char === BOLD_END_MARK) {
      flushBuffer();
      bold = false;
      continue;
    }

    buffer += char;
  }

  flushBuffer();
  return result;
}

function formatFieldLine(fieldName: string, value: string): string {
  const plainWithMarkers = stripHtmlPreserveBold(value);
  const truncated = truncateWithMarkers(plainWithMarkers, DISPLAY_CHAR_LIMIT);
  const renderedValue = applyBoldMarkers(truncated);
  return `  ${chalk.gray(fieldName + ':')} ${renderedValue}`;
}

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
    lines.push(formatFieldLine(fieldName, displayValue));
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

  // Create choices for inquirer without pre-selecting any cards
  const choices = cards.map((card, index) => ({
    name: formatCardForDisplay(card, index + 1),
    value: index,
    checked: false,
  }));
  // Dynamically size the page to fit multi-line choice content
  const totalChoiceLines = choices.reduce((sum, choice) => {
    return sum + choice.name.split('\n').length;
  }, 0);
  const pageSize = Math.min(Math.max(totalChoiceLines, 10), 30);

  try {
    const answers = (await inquirer.prompt([
      {
        type: 'checkbox',
        name: 'selectedCards',
        message: 'Choose cards to import:',
        choices,
        pageSize,
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

import { writeFile } from 'fs/promises';
import inquirer from 'inquirer';
import chalk from 'chalk';
import { z } from 'zod';
import type { Command } from './types.js';
import { ankiRequest } from '../anki-connect.js';
import {
  getFieldNamesForModel,
  findModelNamesForDeck,
} from '../anki-schema.js';

interface GenerateInitArgs {
  output?: string;
}

/**
 * Suggests a short key name for an Anki field.
 * Examples: "English" -> "en", "Japanese" -> "jp"
 */
function suggestKeyForField(fieldName: string): string {
  const lower = fieldName.toLowerCase();

  // Common mappings
  const commonMappings: Record<string, string> = {
    english: 'en',
    japanese: 'jp',
    furigana: 'furigana',
    romaji: 'rom',
    context: 'context',
    notes: 'note',
    translation: 'translation',
    front: 'front',
    back: 'back',
    example: 'example',
    meaning: 'meaning',
  };

  if (commonMappings[lower]) {
    return commonMappings[lower];
  }

  // Default: take first 2-3 letters
  if (fieldName.length <= 3) {
    return lower;
  } else if (fieldName.length <= 6) {
    return lower.substring(0, 3);
  } else {
    return lower.substring(0, 4);
  }
}

/**
 * Generates a boilerplate prompt body with instructions and a one-shot example.
 */
function generatePromptBody(fieldKeys: string[]): string {
  // Create an example JSON object
  const exampleJson: Record<string, string> = {};
  for (const key of fieldKeys) {
    exampleJson[key] = `Example value for ${key}`;
  }

  return `You are an expert assistant who creates one excellent Anki flashcard for a vocabulary term.
The term to create a card for is: **{term}**

IMPORTANT: Your output must be a single, valid JSON object and nothing else.
Do not include any explanation, markdown formatting, or additional text.
All field values must be strings.
For fields that require formatting (like lists or emphasis), generate a single string containing well-formed HTML.

Follow the structure and HTML formatting shown in this example precisely:

\`\`\`json
${JSON.stringify(exampleJson, null, 2)}
\`\`\`

Return only valid JSON matching this structure.

Tips for creating high-quality cards:
- Provide clear, concise definitions or translations
- Include natural, contextual examples
- Use HTML tags like <b>, <i>, <ul>, <li> for formatting when helpful
- For language learning: include pronunciation guides if relevant
- Keep the content focused and easy to review`;
}

const command: Command<GenerateInitArgs> = {
  command: 'generate-init [output]',
  describe: 'Create a prompt template file by querying your Anki collection',

  builder: (yargs) => {
    return yargs
      .positional('output', {
        describe: 'Path to save the prompt file',
        type: 'string',
        default: 'generate-prompt.md',
      })
      .example('$0 generate-init', 'Create prompt.md in current directory')
      .example('$0 generate-init my-prompt.md', 'Save to custom location');
  },

  handler: async (argv) => {
    try {
      console.log(chalk.cyan('\n✨ Welcome to anki-llm generate-init!\n'));
      console.log(
        chalk.gray(
          'This wizard will help you create a prompt template for generating Anki cards.\n',
        ),
      );

      // Check if we're in a TTY environment
      if (!process.stdout.isTTY) {
        console.error(
          chalk.red('❌ This command requires an interactive terminal (TTY).'),
        );
        process.exit(1);
      }

      // Step 1: Fetch and select deck
      console.log(chalk.cyan('📚 Fetching your Anki decks...\n'));
      const deckNames = await ankiRequest('deckNames', z.array(z.string()), {});

      if (deckNames.length === 0) {
        console.error(
          chalk.red('❌ No decks found in Anki. Create a deck first.'),
        );
        process.exit(1);
      }

      const { selectedDeck } = await inquirer.prompt<{
        selectedDeck: string;
      }>([
        {
          type: 'list',
          name: 'selectedDeck',
          message: 'Select the target deck:',
          choices: deckNames,
          pageSize: 15,
        },
      ]);

      console.log(chalk.green(`\n✓ Selected deck: ${selectedDeck}\n`));

      // Step 2: Fetch note types used in the selected deck
      console.log(chalk.cyan('📋 Fetching note types used in this deck...\n'));
      let modelNameChoices = await findModelNamesForDeck(selectedDeck);

      if (modelNameChoices.length === 0) {
        console.log(
          chalk.yellow(
            `⚠️  Deck "${selectedDeck}" has no notes. Showing all available note types instead.\n`,
          ),
        );

        // Fallback to all note types if deck is empty
        modelNameChoices = await ankiRequest(
          'modelNames',
          z.array(z.string()),
          {},
        );

        if (modelNameChoices.length === 0) {
          console.error(
            chalk.red('❌ No note types found in your Anki collection.'),
          );
          process.exit(1);
        }
      }

      let selectedNoteType: string;

      if (modelNameChoices.length === 1) {
        selectedNoteType = modelNameChoices[0];
        console.log(
          chalk.green(
            `✓ Auto-selected the only available note type: ${selectedNoteType}\n`,
          ),
        );
      } else {
        const answer = await inquirer.prompt<{
          selectedNoteType: string;
        }>([
          {
            type: 'list',
            name: 'selectedNoteType',
            message: 'Select the note type:',
            choices: modelNameChoices,
            pageSize: 15,
          },
        ]);
        selectedNoteType = answer.selectedNoteType;
        console.log(
          chalk.green(`\n✓ Selected note type: ${selectedNoteType}\n`),
        );
      }

      // Step 3: Fetch field names and create mapping
      console.log(chalk.cyan('🔍 Fetching fields...\n'));
      const fieldNames = await getFieldNamesForModel(selectedNoteType);

      console.log(
        chalk.gray(
          `Found ${fieldNames.length} field(s): ${fieldNames.join(', ')}\n`,
        ),
      );

      // Step 4: Create field mapping with auto-suggestion and review
      console.log(
        chalk.cyan(
          '🗺️  Creating field mapping (LLM JSON keys → Anki fields)...\n',
        ),
      );

      // Auto-suggest keys for all fields
      const suggestedKeys = fieldNames.map(suggestKeyForField);

      // Detect and resolve duplicate keys
      const keyCounts: Record<string, number> = {};
      const resolvedKeys = suggestedKeys.map((key) => {
        const count = keyCounts[key] || 0;
        keyCounts[key] = count + 1;
        if (count > 0) {
          return `${key}${count + 1}`; // e.g., exp, exp2, exp3
        }
        return key;
      });

      // Build initial fieldMap
      const fieldMap: Record<string, string> = {};
      for (let i = 0; i < fieldNames.length; i++) {
        fieldMap[resolvedKeys[i]] = fieldNames[i];
      }

      // Display proposed mapping
      console.log(chalk.gray('Proposed mapping:'));
      for (const [key, value] of Object.entries(fieldMap)) {
        console.log(chalk.gray(`  ${key} → ${value}`));
      }

      // Ask user to accept or customize
      const { acceptMapping } = await inquirer.prompt<{
        acceptMapping: boolean;
      }>([
        {
          type: 'confirm',
          name: 'acceptMapping',
          message: 'Accept this mapping?',
          default: true,
        },
      ]);

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

          const { jsonKey } = await inquirer.prompt<{ jsonKey: string }>([
            {
              type: 'input',
              name: 'jsonKey',
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
            },
          ]);
          customFieldMap[jsonKey.trim()] = fieldName;
        }

        // Replace with custom mapping
        Object.assign(fieldMap, {});
        Object.assign(fieldMap, customFieldMap);
      }

      console.log(chalk.green('\n✓ Field mapping complete\n'));

      // Step 5: Generate the prompt file content
      console.log(chalk.cyan('\n📝 Generating prompt file...\n'));

      const frontmatter = `---
deck: ${selectedDeck}
noteType: ${selectedNoteType}
fieldMap:
${Object.entries(fieldMap)
  .map(([key, value]) => `  ${key}: ${value}`)
  .join('\n')}
---`;

      const body = generatePromptBody(Object.keys(fieldMap));

      const fullContent = `${frontmatter}\n\n${body}\n`;

      // Step 6: Save the file
      const outputPath = argv.output || 'generate-prompt.md';
      await writeFile(outputPath, fullContent, 'utf-8');

      console.log(chalk.green(`✓ Prompt template saved to ${outputPath}\n`));

      // Step 7: Show example command
      console.log(chalk.cyan('🎉 Setup complete!\n'));
      console.log(chalk.gray('Try it out:'));
      console.log(
        chalk.white(`  anki-llm generate "example term" -p ${outputPath}\n`),
      );
      console.log(
        chalk.gray(
          'Edit the prompt file to customize the instructions for the LLM.',
        ),
      );
    } catch (error) {
      if (error instanceof Error) {
        // Handle user cancellation gracefully
        if (error.message.includes('User force closed')) {
          console.log(chalk.yellow('\n\nWizard cancelled by user.'));
          process.exit(0);
        }
        console.error(chalk.red(`\n❌ Error: ${error.message}`));
      } else {
        console.error(chalk.red('\n❌ An unknown error occurred'));
      }

      console.log(chalk.gray('\nMake sure:'));
      console.log(
        chalk.gray('  1. Anki Desktop is running with AnkiConnect installed'),
      );
      console.log(chalk.gray('  2. You have at least one deck and note type'));

      process.exit(1);
    }
  },
};

export default command;

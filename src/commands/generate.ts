import { readFile } from 'fs/promises';
import chalk from 'chalk';
import { z } from 'zod';
import type { Command } from './types.js';
import { parseFrontmatter } from '../utils/parse-frontmatter.js';
import { sanitizeFields } from '../utils/sanitize-html.js';
import {
  generateCards,
  type GenerationConfig,
} from '../generation/processor.js';
import { validateCards } from '../generation/validator.js';
import { selectCards, displayCards } from '../generation/selector.js';
import { ankiRequest } from '../anki-connect.js';
import { getFieldNamesForModel } from '../anki-schema.js';
import { parseConfig } from '../config.js';

interface GenerateArgs {
  term: string;
  prompt?: string;
  count: number;
  model: string;
  'dry-run': boolean;
  'batch-size': number;
  retries: number;
  'max-tokens'?: number;
  temperature: number;
}

const command: Command<GenerateArgs> = {
  command: 'generate <term>',
  describe: 'Generate Anki cards for a term using an LLM',

  builder: (yargs) => {
    return yargs
      .positional('term', {
        describe: 'Word or phrase to generate cards for',
        type: 'string',
        demandOption: true,
      })
      .option('prompt', {
        alias: 'p',
        describe: 'Path to prompt template file with frontmatter',
        type: 'string',
      })
      .option('count', {
        alias: 'c',
        describe: 'Number of card examples to generate',
        type: 'number',
        default: 3,
      })
      .option('model', {
        alias: 'm',
        describe: 'LLM model to use',
        type: 'string',
        default: 'gpt-4o-mini',
      })
      .option('dry-run', {
        describe: 'Display cards without importing to Anki',
        type: 'boolean',
        default: false,
      })
      .option('batch-size', {
        alias: 'b',
        describe: 'Number of concurrent API requests',
        type: 'number',
        default: 5,
      })
      .option('retries', {
        alias: 'r',
        describe: 'Number of retries for failed requests',
        type: 'number',
        default: 3,
      })
      .option('max-tokens', {
        describe: 'Maximum tokens per response',
        type: 'number',
      })
      .option('temperature', {
        alias: 't',
        describe: 'LLM temperature (0-2)',
        type: 'number',
        default: 1.0,
      })
      .example('$0 generate "今日" -p prompt.md', 'Generate cards for a term')
      .example(
        '$0 generate "hello" -p prompt.md --count 5',
        'Generate 5 examples',
      )
      .example(
        '$0 generate "test" -p prompt.md --dry-run',
        'Preview without importing',
      );
  },

  handler: async (argv) => {
    try {
      // Step 1: Load and parse the prompt file
      if (!argv.prompt) {
        console.error(chalk.red('❌ Error: --prompt (-p) option is required'));
        console.log('\nExample:\n  anki-llm generate "your-term" -p prompt.md');
        console.log(
          '\nTo create a prompt template, run:\n  anki-llm generate-init',
        );
        process.exit(1);
      }

      console.log(chalk.cyan(`\n📖 Loading prompt file: ${argv.prompt}`));
      const promptFileContent = await readFile(argv.prompt, 'utf-8');
      const { frontmatter, body } = parseFrontmatter(promptFileContent);

      console.log(chalk.green(`✓ Loaded prompt for deck: ${frontmatter.deck}`));
      console.log(chalk.green(`✓ Note type: ${frontmatter.noteType}`));

      // Step 2: Validate deck and note type existence
      console.log(chalk.cyan('\n🔍 Validating Anki configuration...'));

      // Check if deck exists
      const deckNames = await ankiRequest('deckNames', z.array(z.string()), {});
      if (!deckNames.includes(frontmatter.deck)) {
        console.error(
          chalk.red(`❌ Deck "${frontmatter.deck}" does not exist in Anki.`),
        );
        console.log(
          chalk.yellow('\nAvailable decks:\n  ' + deckNames.join('\n  ')),
        );
        console.log(
          chalk.gray(
            '\nCreate the deck in Anki first, or update the prompt file.',
          ),
        );
        process.exit(1);
      }

      // Check if note type exists and get field names
      let noteTypeFields: string[];
      try {
        noteTypeFields = await getFieldNamesForModel(frontmatter.noteType);
      } catch {
        console.error(
          chalk.red(`❌ Note type "${frontmatter.noteType}" does not exist.`),
        );
        const modelNames = await ankiRequest(
          'modelNames',
          z.array(z.string()),
          {},
        );
        console.log(
          chalk.yellow('\nAvailable note types:\n  ' + modelNames.join('\n  ')),
        );
        console.log(chalk.gray('\nUpdate the note type in your prompt file.'));
        process.exit(1);
      }

      console.log(
        chalk.green(`✓ Note type fields: ${noteTypeFields.join(', ')}`),
      );

      // Validate that fieldMap target fields exist in the note type
      const mappedFields = Object.values(frontmatter.fieldMap);
      const invalidFields = mappedFields.filter(
        (f) => !noteTypeFields.includes(f),
      );

      if (invalidFields.length > 0) {
        console.error(
          chalk.red(
            `❌ The following fields in your fieldMap do not exist in note type "${frontmatter.noteType}":`,
          ),
        );
        console.log(chalk.red('  ' + invalidFields.join(', ')));
        console.log(
          chalk.yellow(
            '\nUpdate the fieldMap in your prompt file to match the note type fields.',
          ),
        );
        process.exit(1);
      }

      // Parse config (for API keys and model validation)
      const config = parseConfig({
        model: argv.model,
        batchSize: argv['batch-size'],
        maxTokens: argv['max-tokens'],
        temperature: argv.temperature,
        retries: argv.retries,
        dryRun: argv['dry-run'],
        requireResultTag: false, // Not used for generation
      });

      // Step 3: Generate cards
      const generationConfig: GenerationConfig = {
        apiKey: config.apiKey,
        apiBaseUrl: config.apiBaseUrl,
        model: config.model,
        temperature: config.temperature,
        maxTokens: config.maxTokens,
        retries: config.retries,
        batchSize: config.batchSize,
      };

      const { successful, failed } = await generateCards(
        argv.term,
        body,
        argv.count,
        generationConfig,
        frontmatter.fieldMap,
      );

      // Step 4: Handle complete failure
      if (successful.length === 0) {
        console.error(
          chalk.red(`\n❌ All ${argv.count} generation attempts failed.`),
        );
        console.log(chalk.yellow('\nFailure details:'));
        failed.forEach((f, i) => {
          console.log(chalk.gray(`  ${i + 1}. ${f.error.message}`));
        });
        process.exit(1);
      }

      // Step 5: Sanitize HTML in all generated cards
      console.log(chalk.cyan('\n🧹 Sanitizing HTML content...'));
      const sanitizedCards = successful.map((card) => ({
        ...card,
        fields: sanitizeFields(card.fields),
      }));
      console.log(chalk.green('✓ HTML sanitization complete'));

      // Step 6: Validate cards and check for duplicates
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

      // Step 7: Dry run mode - display and exit
      if (argv['dry-run']) {
        displayCards(validatedCards);
        process.exit(0);
      }

      // Step 8: Interactive selection
      const selectedIndices = await selectCards(validatedCards);
      const selectedCards = selectedIndices.map((i) => validatedCards[i]);

      if (selectedCards.length === 0) {
        console.log(chalk.yellow('\n⚠️  No cards selected. Exiting.'));
        process.exit(0);
      }

      // Step 9: Import to Anki
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

      // Step 10: Report outcome
      if (failures > 0) {
        console.log(
          chalk.yellow(`\n⚠️  Added ${successes} card(s), ${failures} failed`),
        );
        console.log(
          chalk.gray(
            'Some cards may have been duplicates or had invalid field values.',
          ),
        );
        process.exit(1);
      } else {
        console.log(
          chalk.green(
            `\n✓ Successfully added ${successes} new note(s) to "${frontmatter.deck}"`,
          ),
        );
        process.exit(0);
      }
    } catch (error) {
      if (error instanceof Error) {
        console.error(chalk.red(`\n❌ Error: ${error.message}`));
      } else {
        console.error(chalk.red('\n❌ An unknown error occurred'));
      }

      console.log(chalk.gray('\nMake sure:'));
      console.log(
        chalk.gray('  1. Anki Desktop is running with AnkiConnect installed'),
      );
      console.log(chalk.gray('  2. Your prompt file is correctly formatted'));
      console.log(chalk.gray('  3. The deck and note type exist in Anki'));

      process.exit(1);
    }
  },
};

export default command;

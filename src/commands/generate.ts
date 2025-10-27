import { readFile } from 'fs/promises';
import chalk from 'chalk';
import type { Command } from './types.js';
import { parseFrontmatter } from '../utils/parse-frontmatter.js';
import { generateCards } from '../generation/processor.js';
import { SupportedModel, parseConfig } from '../config.js';
import { readConfigFile } from '../config-manager.js';
import { validateAnkiAssets } from '../generate/anki-validation.js';
import { processAndSelectCards } from '../generate/card-processing.js';
import {
  importCardsToAnki,
  reportImportResult,
} from '../generate/anki-import.js';

interface GenerateArgs {
  term: string;
  prompt?: string;
  count: number;
  model?: string;
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
        describe: `LLM model to use. Available: ${SupportedModel.options.join(', ')}`,
        type: 'string',
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

      // Step 2: Validate Anki assets
      console.log(chalk.cyan('\n🔍 Validating Anki configuration...'));
      const { noteTypeFields } = await validateAnkiAssets(frontmatter);
      console.log(
        chalk.green(`✓ Note type fields: ${noteTypeFields.join(', ')}`),
      );

      // Step 3: Resolve configuration
      const userConfig = await readConfigFile();
      const model = argv.model ?? userConfig.model;

      if (!model) {
        console.error(
          chalk.red(
            '❌ A model must be specified via the --model flag or set in the configuration.',
          ),
        );
        console.error(
          chalk.dim(`Available models: ${SupportedModel.options.join(', ')}`),
        );
        process.exit(1);
      }

      const appConfig = parseConfig({
        model,
        batchSize: argv['batch-size'],
        maxTokens: argv['max-tokens'],
        temperature: argv.temperature,
        retries: argv.retries,
        dryRun: argv['dry-run'],
        requireResultTag: false, // Not used by generate command
      });

      // Step 4: Generate cards
      const { successful, failed } = await generateCards(
        argv.term,
        body,
        argv.count,
        appConfig,
        frontmatter.fieldMap,
      );

      // Handle complete failure
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

      // Step 5: Process and select cards
      const selectedCards = await processAndSelectCards(
        successful,
        frontmatter,
        noteTypeFields,
        argv['dry-run'],
      );

      // Step 6: Handle dry-run or no selection
      if (argv['dry-run']) {
        console.log(chalk.cyan('\nDry run complete. No cards were imported.'));
        process.exit(0);
      }
      if (selectedCards.length === 0) {
        console.log(chalk.yellow('\n⚠️  No cards selected. Exiting.'));
        process.exit(0);
      }

      // Step 7: Import to Anki and report
      const importResult = await importCardsToAnki(selectedCards, frontmatter);
      reportImportResult(importResult, frontmatter.deck);

      if (importResult.failures > 0) {
        process.exit(1);
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

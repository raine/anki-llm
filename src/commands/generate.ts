import { readFile } from 'fs/promises';
import chalk from 'chalk';
import { z } from 'zod';
import type { Command } from './types.js';
import { parseFrontmatter } from '../utils/parse-frontmatter.js';
import { generateCards } from '../generate/processor.js';
import { SupportedModel, parseConfig } from '../config.js';
import { readConfigFile } from '../config-manager.js';
import { validateAnkiAssets } from '../generate/anki-validation.js';
import { processAndSelectCards } from '../generate/card-processing.js';
import { exportCards } from '../generate/exporter.js';
import {
  importCardsToAnki,
  reportImportResult,
} from '../generate/anki-import.js';
import { formatCostDisplay } from '../utils/llm-cost.js';
import { getLlmResponseManually } from '../utils/manual-llm.js';
import { fillTemplate } from '../batch-processing/util.js';
import { parseLlmJson } from '../utils/parse-llm-json.js';
import type { CardCandidate } from '../types.js';

interface GenerateArgs {
  term: string;
  prompt?: string;
  count: number;
  model?: string;
  'dry-run': boolean;
  retries: number;
  'max-tokens'?: number;
  temperature: number;
  output?: string;
  log: boolean;
  copy: boolean;
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
      .option('output', {
        alias: 'o',
        describe:
          'Export cards to a file instead of importing to Anki (e.g., cards.yaml, cards.csv)',
        type: 'string',
      })
      .option('log', {
        describe:
          'Enable logging of LLM responses to a file (useful for debugging)',
        type: 'boolean',
        default: false,
      })
      .option('copy', {
        describe:
          'Copy the LLM prompt to clipboard and wait for manual response pasting',
        type: 'boolean',
        default: false,
      })
      .example('$0 generate "今日" -p prompt.md', 'Generate cards for a term')
      .example(
        '$0 generate "hello" -p prompt.md --count 5',
        'Generate 5 examples',
      )
      .example(
        '$0 generate "test" -p prompt.md --dry-run',
        'Preview without importing',
      )
      .example(
        '$0 generate "今日" -p prompt.md -o cards.yaml',
        'Export cards to YAML',
      )
      .example(
        '$0 generate "今日" -p prompt.md --log',
        'Generate with response logging',
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
      let model = argv.model ?? userConfig.model;

      // If no model specified, use defaults
      if (!model) {
        const useGemini =
          !process.env.OPENAI_API_KEY && process.env.GEMINI_API_KEY;
        model = useGemini ? 'gemini-2.5-flash' : 'gpt-5-mini';
        console.log(
          chalk.gray(
            `ℹ️  Using default model: ${model} (override with --model)`,
          ),
        );
      }

      const appConfig = parseConfig({
        model,
        maxTokens: argv['max-tokens'],
        temperature: argv.temperature,
        retries: argv.retries,
        dryRun: argv['dry-run'],
        requireResultTag: false, // Not used by generate command
      });

      // Step 4: Setup logging if requested
      let logFilePath: string | undefined;
      if (argv.log) {
        const timestamp = new Date()
          .toISOString()
          .replace(/[:.]/g, '-')
          .replace('T', '_')
          .substring(0, 19);
        logFilePath = `generate-${timestamp}.log`;
      }

      // Step 5: Generate cards
      let successful: CardCandidate[];

      if (argv.copy) {
        // Manual copy-paste flow
        if (!process.stdout.isTTY) {
          throw new Error('--copy mode requires an interactive terminal.');
        }
        console.log(chalk.cyan('\n📋 Running in manual copy mode.'));

        const filledPrompt = fillTemplate(body, {
          term: argv.term,
          count: String(argv.count),
        });

        const rawResponse = await getLlmResponseManually(filledPrompt);

        try {
          // Replicate parsing logic from processor.ts
          const CardObjectSchema = z.object(
            Object.keys(frontmatter.fieldMap).reduce(
              (acc, key) => {
                acc[key] = z.string();
                return acc;
              },
              {} as Record<string, z.ZodString>,
            ),
          );
          const CardArraySchema = z.array(CardObjectSchema);

          const parsed = parseLlmJson(rawResponse);
          const validatedCards = CardArraySchema.parse(parsed);
          successful = validatedCards.map((cardFields) => ({
            fields: cardFields,
            rawResponse,
          }));

          console.log(
            chalk.green(
              `✓ Successfully parsed ${successful.length} card(s) from response`,
            ),
          );
        } catch (parseError) {
          const message =
            parseError instanceof Error
              ? parseError.message
              : String(parseError);
          console.error(chalk.red('\n❌ Failed to parse the pasted response:'));
          console.error(chalk.gray(message));
          process.exit(1);
        }
      } else {
        // API-based flow
        const {
          successful: apiSuccessful,
          failed,
          costInfo,
        } = await generateCards(
          argv.term,
          body,
          argv.count,
          appConfig,
          frontmatter.fieldMap,
          logFilePath,
        );
        successful = apiSuccessful;

        // Report cost if available
        if (costInfo) {
          console.log(
            formatCostDisplay(
              costInfo.totalCost,
              costInfo.inputTokens,
              costInfo.outputTokens,
            ),
          );
        }

        // Handle complete failure from API
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
      }

      // Step 6: Process and select cards
      const selectedCards = await processAndSelectCards(
        successful,
        frontmatter,
        noteTypeFields,
        argv['dry-run'],
      );

      // Step 7: Handle dry-run or no selection
      if (argv['dry-run']) {
        console.log(
          chalk.cyan('\nDry run complete. No cards were imported or exported.'),
        );
        process.exit(0);
      }
      if (selectedCards.length === 0) {
        console.log(chalk.yellow('\n⚠️  No cards selected. Exiting.'));
        process.exit(0);
      }

      // Step 8: Export or import to Anki
      if (argv.output) {
        await exportCards(selectedCards, argv.output);
      } else {
        const importResult = await importCardsToAnki(
          selectedCards,
          frontmatter,
        );
        reportImportResult(importResult, frontmatter.deck);

        if (importResult.failures > 0) {
          process.exit(1);
        }
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

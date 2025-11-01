import { writeFile } from 'fs/promises';
import chalk from 'chalk';
import type { Command } from './types.js';
import { slugifyDeckName } from '../batch-processing/util.js';
import {
  selectDeck,
  selectNoteType,
  configureFieldMapping,
} from '../generate-init/interactive-steps.js';
import { createPromptContent } from '../generate-init/prompt-generation.js';
import { formatCostDisplay } from '../utils/llm-cost.js';

interface GenerateInitArgs {
  output?: string;
  model?: string;
  temperature?: number;
}

const command: Command<GenerateInitArgs> = {
  command: 'generate-init [output]',
  describe: 'Create a prompt template file by querying your Anki collection',

  builder: (yargs) => {
    return yargs
      .positional('output', {
        describe:
          'Path to save the prompt file (defaults to deck-name-prompt.md)',
        type: 'string',
      })
      .option('model', {
        alias: 'm',
        describe:
          'LLM model to use for prompt generation (e.g., gpt-4o, gemini-2.5-flash)',
        type: 'string',
      })
      .option('temperature', {
        alias: 't',
        describe:
          'Temperature for LLM generation (0.0-2.0, default varies by model)',
        type: 'number',
      })
      .example('$0 generate-init', 'Create prompt file named after the deck')
      .example('$0 generate-init my-prompt.md', 'Save to custom location')
      .example(
        '$0 generate-init --model gemini-2.5-flash',
        'Use Gemini for prompt generation',
      )
      .example(
        '$0 generate-init --temperature 0.5',
        'Use lower temperature for more consistent output',
      );
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

      const selectedDeck = await selectDeck();
      const selectedNoteType = await selectNoteType(selectedDeck);
      const initialFieldMap = await configureFieldMapping(selectedNoteType);

      console.log(
        chalk.cyan(
          '\n🧠 Attempting to generate a smart prompt based on your existing cards...\n',
        ),
      );
      console.log(
        chalk.gray(
          '(This requires OPENAI_API_KEY or GEMINI_API_KEY environment variable)\n',
        ),
      );

      const { body, finalFieldMap, costInfo } = await createPromptContent(
        selectedDeck,
        initialFieldMap,
        argv.model,
        argv.temperature,
      );

      // Report cost if available
      if (costInfo) {
        console.log(
          formatCostDisplay(
            costInfo.totalCost,
            costInfo.inputTokens,
            costInfo.outputTokens,
          ) + '\n',
        );
      }

      console.log(chalk.cyan('📝 Creating prompt file...\n'));

      const frontmatter = `---
deck: ${selectedDeck}
noteType: ${selectedNoteType}
fieldMap:
${Object.entries(finalFieldMap)
  .map(([key, value]) => `  ${key}: ${value}`)
  .join('\n')}
---`;

      const fullContent = `${frontmatter}\n\n${body}\n`;

      const defaultFilename = `${slugifyDeckName(selectedDeck)}-prompt.md`;
      const outputPath = argv.output || defaultFilename;
      await writeFile(outputPath, fullContent, 'utf-8');

      console.log(chalk.green(`✓ Prompt template saved to ${outputPath}\n`));

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

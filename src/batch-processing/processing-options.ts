import { Argv } from 'yargs';
import { SupportedModel } from '../config.js';

export function addSharedOptions<T>(yargs: Argv<T>) {
  return yargs
    .option('field', {
      describe:
        'Field name to update with AI response (mutually exclusive with --json)',
      type: 'string',
    })
    .option('json', {
      describe:
        'Expect JSON response and merge all fields (mutually exclusive with --field)',
      type: 'boolean',
      default: false,
    })
    .option('prompt', {
      alias: 'p',
      describe: 'Path to prompt template file',
      type: 'string',
      demandOption: true,
    })
    .option('model', {
      alias: 'm',
      describe: `Model to use. Available: ${SupportedModel.options.join(', ')}`,
      type: 'string',
    })
    .option('batch-size', {
      alias: 'b',
      describe: 'Number of concurrent API requests',
      type: 'number',
      default: 5,
    })
    .option('max-tokens', {
      describe: 'Maximum tokens for completion',
      type: 'number',
    })
    .option('temperature', {
      alias: 't',
      describe: 'Temperature for model sampling',
      type: 'number',
      default: 0.0,
    })
    .option('retries', {
      alias: 'r',
      describe: 'Number of retries for failed requests',
      type: 'number',
      default: 3,
    })
    .option('dry-run', {
      alias: 'd',
      describe: 'Preview operation without making API calls',
      type: 'boolean',
      default: false,
    })
    .option('require-result-tag', {
      describe: 'Require <result> tags in AI responses',
      type: 'boolean',
      default: false,
    })
    .option('limit', {
      describe: 'Limit the number of notes/rows to process (for testing)',
      type: 'number',
    })
    .option('log', {
      describe: 'Generate a log file',
      type: 'boolean',
      default: false,
    })
    .option('very-verbose', {
      describe: 'Log LLM responses to log file (automatically enables --log)',
      type: 'boolean',
      default: false,
    })
    .check((argv) => {
      if (argv.limit !== undefined && argv.limit <= 0) {
        throw new Error('Error: --limit must be a positive number.');
      }
      // Require either --field or --json (but not both)
      if (!argv.field && !argv.json) {
        throw new Error('Error: Either --field or --json must be specified.');
      }
      if (argv.field && argv.json) {
        throw new Error(
          'Error: --field and --json are mutually exclusive. Use only one.',
        );
      }
      return true;
    });
}

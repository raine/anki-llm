import yargs from 'yargs';
import { hideBin } from 'yargs/helpers';

/**
 * Parses command-line arguments for batch AI processing
 */
export function parseCliArgs() {
  return yargs(hideBin(process.argv))
    .usage('Usage: $0 <input> <output> <field> <prompt>')
    .command(
      '$0 <input> <output> <field> <prompt>',
      'Process a field in a CSV/YAML file using AI',
    )
    .positional('input', {
      describe: 'Input file path (CSV or YAML)',
      type: 'string',
      demandOption: true,
    })
    .positional('output', {
      describe: 'Output file path (CSV or YAML)',
      type: 'string',
      demandOption: true,
    })
    .positional('field', {
      describe: 'Field name to process with AI',
      type: 'string',
      demandOption: true,
    })
    .positional('prompt', {
      describe: 'Path to prompt template file',
      type: 'string',
      demandOption: true,
    })
    .option('model', {
      alias: 'm',
      describe: 'AI model to use',
      type: 'string',
      default: 'gpt-4o-mini',
      choices: [
        'gpt-4.1',
        'gpt-4o',
        'gpt-4o-mini',
        'gpt-5-nano',
        'gemini-2.0-flash',
        'gemini-2.5-flash',
        'gemini-2.5-flash-lite',
      ],
    })
    .option('batch-size', {
      alias: 'b',
      describe: 'Number of concurrent requests',
      type: 'number',
      default: 5,
    })
    .option('max-tokens', {
      describe: 'Maximum tokens per completion',
      type: 'number',
    })
    .option('temperature', {
      alias: 't',
      describe: 'Sampling temperature (0-2)',
      type: 'number',
      default: 0.3,
    })
    .option('retries', {
      alias: 'r',
      describe: 'Number of retries on failure',
      type: 'number',
      default: 3,
    })
    .option('dry-run', {
      alias: 'd',
      describe: 'Preview without making changes',
      type: 'boolean',
      default: false,
    })
    .option('force', {
      alias: 'f',
      describe: 'Force re-processing of all rows (ignore existing output)',
      type: 'boolean',
      default: false,
    })
    .option('require-result-tag', {
      describe:
        'Require <result></result> XML tags in responses (fail if missing)',
      type: 'boolean',
      default: false,
    })
    .example(
      '$0 input.csv output.csv english prompt.txt',
      'Process english field with defaults',
    )
    .example(
      '$0 input.yaml out.yaml text prompt.txt -m gpt-4o -t 0.7 -b 10',
      'Custom model, temperature, and batch size',
    )
    .example(
      '$0 data.csv result.csv field prompt.txt --dry-run',
      'Preview without processing',
    )
    .example(
      '$0 data.yaml out.yaml field prompt.txt --force',
      'Re-process all rows (ignore existing output)',
    )
    .epilogue(
      'Environment variables:\n' +
        '  OPENAI_API_KEY or GEMINI_API_KEY - Required: API key for LLM provider',
    )
    .help()
    .parseSync();
}

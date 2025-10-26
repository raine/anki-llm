import { readFile } from 'fs/promises';
import chalk from 'chalk';
import OpenAI from 'openai';
import type { Argv } from 'yargs';
import { SupportedModel, type Config } from '../config.js';
import { readConfigFile } from '../config-manager.js';
import { initLogger, logDebug, logInfo } from '../batch-processing/logger.js';
import { printSummary } from '../batch-processing/reporting.js';
import type {
  ProcessedRow,
  RowData,
  TokenStats,
} from '../batch-processing/types.js';
import { fillTemplate } from '../batch-processing/util.js';

export interface CommonProcessingArgs {
  field?: string;
  json: boolean;
  prompt: string;
  model?: string;
  'batch-size': number;
  'max-tokens'?: number;
  temperature: number;
  retries: number;
  'dry-run': boolean;
  'require-result-tag': boolean;
  limit?: number;
  log: boolean;
  'very-verbose': boolean;
}

export function applyCommonProcessingOptions<T>(
  yargs: Argv<T>,
  options: { limitDescription: string },
): Argv<T & CommonProcessingArgs> {
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
      describe: options.limitDescription,
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
      const { field, json, limit } = argv as unknown as CommonProcessingArgs;
      if (limit !== undefined && limit <= 0) {
        throw new Error('Error: --limit must be a positive number.');
      }
      if (!field && !json) {
        throw new Error('Error: Either --field or --json must be specified.');
      }
      if (field && json) {
        throw new Error(
          'Error: --field and --json are mutually exclusive. Use only one.',
        );
      }
      return true;
    });
}

export async function resolveModelOrExit(argvModel?: string): Promise<string> {
  const userConfig = await readConfigFile();
  const storedModel =
    typeof userConfig.model === 'string' ? userConfig.model : undefined;
  const model = argvModel ?? storedModel;

  if (!model) {
    console.log(
      chalk.red(
        '✗ Error: A model must be specified via the --model flag or set in the configuration.',
      ),
    );
    console.log(
      chalk.dim(
        '\nTo set a default model, run: anki-llm-batch config set model <model-name>',
      ),
    );
    console.log(
      chalk.dim(`Available models: ${SupportedModel.options.join(', ')}`),
    );
    process.exit(1);
  }

  return model;
}

export async function setupProcessingLogger(options: {
  enabled: boolean;
  getLogFilePath: () => string;
  sessionName: string;
  veryVerbose: boolean;
}): Promise<string | null> {
  if (!options.enabled) {
    return null;
  }

  const logFilePath = options.getLogFilePath();
  await initLogger(logFilePath, options.veryVerbose);
  await logDebug('='.repeat(60));
  await logDebug(`${options.sessionName} - Session Started`);
  await logDebug('='.repeat(60));
  if (options.veryVerbose) {
    await logDebug('Very verbose mode enabled - will log LLM responses');
  }

  return logFilePath;
}

export async function loadPromptTemplate(promptPath: string): Promise<string> {
  const promptTemplate = await readFile(promptPath, 'utf-8');
  await logDebug(`Prompt template loaded (${promptTemplate.length} chars)`);
  return promptTemplate;
}

export function printProcessingHeader(options: {
  title: string;
  extraLines: string[];
  logFilePath: string | null;
  jsonMode: boolean;
  fieldName?: string;
  config: Config;
}): void {
  const { title, extraLines, logFilePath, jsonMode, fieldName, config } =
    options;

  logInfo(chalk.bold('='.repeat(60)));
  logInfo(chalk.bold(title));
  logInfo(chalk.bold('='.repeat(60)));
  for (const line of extraLines) {
    logInfo(line);
  }
  if (logFilePath) {
    logInfo(`Log file:          ${logFilePath}`);
  }
  if (jsonMode) {
    logInfo('Mode:              JSON merge');
  } else {
    logInfo(`Field to process:  ${fieldName}`);
  }
  logInfo(`Model:             ${config.model}`);
  logInfo(`Batch size:        ${config.batchSize}`);
  logInfo(`Retries:           ${config.retries}`);
  logInfo(`Temperature:       ${config.temperature}`);
  if (config.maxTokens) {
    logInfo(`Max tokens:        ${config.maxTokens}`);
  }
  logInfo(`Dry run:           ${config.dryRun}`);
  logInfo(`Require result tag: ${config.requireResultTag}`);
  logInfo(chalk.bold('='.repeat(60)));
}

export async function maybeHandleDryRun<T extends RowData>(options: {
  config: Config;
  rows: T[];
  promptTemplate: string;
  itemLabel: string;
  sampleLabel: string;
  dryRunMessage: string;
}): Promise<boolean> {
  const {
    config,
    rows,
    promptTemplate,
    itemLabel,
    sampleLabel,
    dryRunMessage,
  } = options;

  if (!config.dryRun) {
    return false;
  }

  logInfo(chalk.yellow(`\n⚠️  DRY RUN MODE - ${dryRunMessage}`));
  logInfo(`Would process ${rows.length} ${itemLabel}`);
  logInfo(`\n${chalk.bold('Prompt template:')}`);
  logInfo(promptTemplate);

  if (rows.length > 0) {
    const sample = rows[0];
    logInfo(`\n${chalk.bold(sampleLabel)}`);
    logInfo(JSON.stringify(sample, null, 2));
    logInfo(`\n${chalk.bold('Sample prompt:')}`);
    logInfo(fillTemplate(promptTemplate, sample));
  }

  await logDebug('Dry run complete. Exiting.');
  return true;
}

export function createOpenAIClient(config: Config): OpenAI {
  return new OpenAI({
    apiKey: config.apiKey,
    ...(config.apiBaseUrl && { baseURL: config.apiBaseUrl }),
  });
}

export async function finalizeProcessing(options: {
  processedRows: ProcessedRow[];
  tokenStats: TokenStats;
  config: Config;
  startTime: number;
  errorLogPath?: string;
}): Promise<number> {
  const { processedRows, tokenStats, config, startTime, errorLogPath } =
    options;
  const elapsedMs = Date.now() - startTime;

  const summaryOptions =
    errorLogPath !== undefined ? { errorLogPath } : undefined;
  printSummary(processedRows, tokenStats, config, elapsedMs, summaryOptions);

  const failures = processedRows.filter((row) => row._error);
  const successes = processedRows.length - failures.length;
  await logDebug(`Summary: ${successes} successful, ${failures.length} failed`);
  await logDebug(
    `Total tokens: ${tokenStats.input + tokenStats.output} (input: ${tokenStats.input}, output: ${tokenStats.output})`,
  );
  await logDebug(`Total time: ${(elapsedMs / 1000).toFixed(2)}s`);
  await logDebug('Session completed successfully');

  return failures.length > 0 ? 1 : 0;
}

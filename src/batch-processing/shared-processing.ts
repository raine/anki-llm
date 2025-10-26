import { readFile } from 'fs/promises';
import chalk from 'chalk';
import { parseConfig, SupportedModel } from '../config.js';
import { readConfigFile } from '../config-manager.js';
import { initLogger, logDebug, logInfo } from '../batch-processing/logger.js';
import { fillTemplate } from '../batch-processing/util.js';
import type { RowData } from '../batch-processing/types.js';

export interface SharedProcessingArgs {
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

export async function commonProcessingHandler<T extends SharedProcessingArgs>(
  argv: T,
  sourceName: string,
  notesToProcess: RowData[],
) {
  const startTime = Date.now();

  // Load config and use as fallback for model
  const userConfig = await readConfigFile();
  const storedModel =
    typeof userConfig.model === 'string' ? userConfig.model : undefined;
  const model = argv.model ?? storedModel;

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

  // Initialize logger
  let logFilePath: string | null = null;
  if (argv.log || argv['very-verbose']) {
    const logFileName = `${sourceName}-process.log`;
    logFilePath = logFileName;
    await initLogger(logFilePath, argv['very-verbose']);
    await logDebug('='.repeat(60));
    await logDebug('Processing Session Started');
    await logDebug('='.repeat(60));
    if (argv['very-verbose']) {
      await logDebug('Very verbose mode enabled - will log LLM responses');
    }
  }

  // Parse configuration
  const config = parseConfig({
    model,
    batchSize: argv['batch-size'],
    maxTokens: argv['max-tokens'],
    temperature: argv.temperature,
    retries: argv.retries,
    dryRun: argv['dry-run'],
    requireResultTag: argv['require-result-tag'],
  });

  // Read prompt template
  const promptTemplate = await readFile(argv.prompt, 'utf-8');
  await logDebug(`Prompt template loaded (${promptTemplate.length} chars)`);

  // Print header
  logInfo(chalk.bold('='.repeat(60)));
  logInfo(chalk.bold('Anki LLM Batch Processing'));
  logInfo(chalk.bold('='.repeat(60)));
  logInfo(`Source:            ${sourceName}`);
  if (logFilePath) {
    logInfo(`Log file:          ${logFilePath}`);
  }
  if (argv.json) {
    logInfo(`Mode:              JSON merge`);
  } else {
    logInfo(`Field to process:  ${argv.field}`);
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

  // Handle dry run
  if (config.dryRun) {
    logInfo(chalk.yellow('\n⚠️  DRY RUN MODE - No changes will be made'));
    logInfo(`Would process ${notesToProcess.length} notes`);
    logInfo(`\n${chalk.bold('Prompt template:')}`);
    logInfo(promptTemplate);
    logInfo(`\n${chalk.bold('Sample note:')}`);
    logInfo(JSON.stringify(notesToProcess[0], null, 2));
    logInfo(`\n${chalk.bold('Sample prompt:')}`);
    logInfo(fillTemplate(promptTemplate, notesToProcess[0]));
    await logDebug('Dry run complete. Exiting.');
    return null;
  }

  return { config, promptTemplate, startTime };
}

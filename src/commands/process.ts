import { readFile } from 'fs/promises';
import OpenAI from 'openai';
import chalk from 'chalk';
import * as path from 'path';
import { parseConfig } from '../config.js';
import {
  initLogger,
  logDebug,
  logInfo,
  logInfoTee,
} from '../batch-processing/logger.js';
import {
  parseDataFile,
  loadExistingOutput,
} from '../batch-processing/data-io.js';
import { processAllRows } from '../batch-processing/processor.js';
import { printSummary } from '../batch-processing/reporting.js';
import { requireNoteId, fillTemplate } from '../batch-processing/util.js';
import type { RowData } from '../batch-processing/types.js';
import { AbortError } from 'p-retry';
import type { Command } from './types.js';

interface BatchArgs {
  input: string;
  output: string;
  field: string;
  prompt: string;
  model: string;
  'batch-size': number;
  'max-tokens'?: number;
  temperature: number;
  retries: number;
  'dry-run': boolean;
  'require-result-tag': boolean;
  force: boolean;
}

const command: Command<BatchArgs> = {
  command: 'process <input>',
  describe: 'Process Anki notes with AI',

  builder: (yargs) => {
    return yargs
      .positional('input', {
        describe: 'Input file path (CSV or YAML)',
        type: 'string',
        demandOption: true,
      })
      .option('output', {
        alias: 'o',
        describe: 'Output file path (CSV or YAML)',
        type: 'string',
        demandOption: true,
      })
      .option('field', {
        describe: 'Field name to process',
        type: 'string',
        demandOption: true,
      })
      .option('prompt', {
        alias: 'p',
        describe: 'Path to prompt template file',
        type: 'string',
        demandOption: true,
      })
      .option('model', {
        alias: 'm',
        describe: 'OpenAI model to use',
        type: 'string',
        default: 'gpt-4o-mini',
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
      .option('force', {
        alias: 'f',
        describe: 'Re-process all rows, ignoring existing output',
        type: 'boolean',
        default: false,
      })
      .example(
        '$0 process input.csv -o output.csv --field Translation -p prompt.txt',
        'Process Translation field',
      )
      .example(
        '$0 process data.yaml -o result.yaml --field Notes -p prompt.txt -m gpt-4',
        'Use GPT-4 model',
      );
  },

  handler: async (argv) => {
    const startTime = Date.now();

    // Initialize logger
    const outputDir = path.dirname(argv.output);
    const outputName = path.basename(argv.output, path.extname(argv.output));
    const logFilePath = path.join(outputDir, `${outputName}.log`);
    await initLogger(logFilePath);

    await logDebug('='.repeat(60));
    await logDebug('Batch AI Data Processing - Session Started');
    await logDebug('='.repeat(60));

    // Parse configuration from CLI args
    const config = parseConfig({
      model: argv.model,
      batchSize: argv['batch-size'],
      maxTokens: argv['max-tokens'],
      temperature: argv.temperature,
      retries: argv.retries,
      dryRun: argv['dry-run'],
      requireResultTag: argv['require-result-tag'],
    });

    await logDebug(`Log file: ${logFilePath}`);
    await logDebug(`Input file: ${argv.input}`);
    await logDebug(`Output file: ${argv.output}`);
    await logDebug(`Field to process: ${argv.field}`);
    await logDebug(`Model: ${config.model}`);
    await logDebug(`Batch size: ${config.batchSize}`);
    await logDebug(`Retries: ${config.retries}`);
    await logDebug(`Temperature: ${config.temperature}`);
    if (config.maxTokens) {
      await logDebug(`Max tokens: ${config.maxTokens}`);
    }
    await logDebug(`Dry run: ${config.dryRun}`);
    await logDebug(`Require result tag: ${config.requireResultTag}`);

    // Read prompt template from file
    const promptTemplate = await readFile(argv.prompt, 'utf-8');
    await logDebug(`Prompt template loaded (${promptTemplate.length} chars)`);

    logInfo(chalk.bold('='.repeat(60)));
    logInfo(chalk.bold('Batch AI Data Processing'));
    logInfo(chalk.bold('='.repeat(60)));
    logInfo(`Input file:        ${argv.input}`);
    logInfo(`Output file:       ${argv.output}`);
    logInfo(`Log file:          ${logFilePath}`);
    logInfo(`Field to process:  ${argv.field}`);
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

    // Read and parse data file
    await logInfoTee(`\n${chalk.cyan('Reading')} ${argv.input}...`);
    const rows = await parseDataFile(argv.input);
    const inputFormat = path.extname(argv.input).substring(1).toUpperCase();
    await logInfoTee(
      chalk.green(`✓ Found ${rows.length} rows in ${inputFormat}`),
    );

    if (rows.length === 0) {
      await logInfoTee(chalk.yellow('No rows to process. Exiting.'));
      return;
    }

    // Validate field exists
    await logDebug('Validating field exists in input data');
    if (rows.length > 0 && !(argv.field in rows[0])) {
      const error = `Field "${argv.field}" not found in input file. Available fields: ${Object.keys(rows[0]).join(', ')}`;
      await logDebug(`ERROR: ${error}`);
      throw new Error(error);
    }
    await logDebug(`Field "${argv.field}" validated successfully`);

    // Validate no duplicate noteIds in input
    await logDebug('Validating no duplicate noteIds in input');
    const seenIds = new Set<string>();
    for (let i = 0; i < rows.length; i++) {
      const noteId = requireNoteId(rows[i]);
      if (seenIds.has(noteId)) {
        const error = `Duplicate noteId "${noteId}" detected in input (first seen at row ${seenIds.size + 1}, duplicate at row ${i + 1})`;
        await logDebug(`ERROR: ${error}`);
        throw new AbortError(error);
      }
      seenIds.add(noteId);
    }
    await logDebug('No duplicate noteIds found');

    // Load existing output and filter rows (always enabled unless --force)
    const force = argv.force;

    let existingRowsMap = new Map<string, RowData>();
    let rowsToProcess = rows;

    if (!force) {
      await logInfoTee(`\n${chalk.cyan('Loading')} existing output...`);
      existingRowsMap = await loadExistingOutput(argv.output);

      if (existingRowsMap.size > 0) {
        await logInfoTee(
          chalk.green(`✓ Found ${existingRowsMap.size} already-processed rows`),
        );

        // Filter out rows that are already processed (skip rows with errors - retry them)
        rowsToProcess = rows.filter((row) => {
          const noteId = requireNoteId(row);
          const existing = existingRowsMap.get(noteId);
          // Process if: no existing row, or existing row has an error
          return !existing || existing._error;
        });

        const skippedCount = rows.length - rowsToProcess.length;
        if (skippedCount > 0) {
          await logInfoTee(
            chalk.yellow(`⏭  Skipping ${skippedCount} already-processed rows`),
          );
        }
      } else {
        await logDebug('No existing output found, will process all rows');
      }
    } else {
      await logDebug('Force mode enabled, will re-process all rows');
    }

    if (rowsToProcess.length === 0) {
      await logInfoTee(
        chalk.green('\n✓ All rows already processed. Nothing to do.'),
      );
      return;
    }

    await logDebug(`Total rows to process: ${rowsToProcess.length}`);

    // Handle dry run mode
    if (config.dryRun) {
      logInfo(chalk.yellow('\n⚠️  DRY RUN MODE - No changes will be saved'));
      logInfo(`Would process ${rowsToProcess.length} rows`);
      logInfo(`\n${chalk.bold('Prompt template:')}`);
      logInfo(promptTemplate);
      logInfo(`\n${chalk.bold('Sample row:')}`);
      logInfo(JSON.stringify(rowsToProcess[0], null, 2));
      logInfo(`\n${chalk.bold('Sample prompt:')}`);
      logInfo(fillTemplate(promptTemplate, rowsToProcess[0]));
      await logDebug('Dry run complete. Exiting.');
      return;
    }

    // Initialize OpenAI client
    await logDebug(`Initializing OpenAI client for model: ${config.model}`);
    const client = new OpenAI({
      apiKey: config.apiKey,
      ...(config.apiBaseUrl && { baseURL: config.apiBaseUrl }),
    });

    // Process remaining rows with incremental writing
    await logInfoTee(
      `\n${chalk.cyan('Processing')} ${rowsToProcess.length} rows...`,
    );
    const { rows: processedRows, tokenStats } = await processAllRows(
      rowsToProcess,
      argv.field,
      promptTemplate,
      config,
      client,
      {
        allRows: rows,
        existingRowsMap: existingRowsMap,
        outputPath: argv.output,
      },
    );

    logInfo(chalk.green(`\n✓ Processing complete`));
    await logDebug('Batch processing complete');

    // Print summary
    const elapsedMs = Date.now() - startTime;
    printSummary(processedRows, tokenStats, config, elapsedMs);

    // Log summary to file
    const failures = processedRows.filter((r) => r._error);
    const successes = processedRows.length - failures.length;
    await logDebug(
      `Summary: ${successes} successful, ${failures.length} failed`,
    );
    await logDebug(
      `Total tokens: ${tokenStats.input + tokenStats.output} (input: ${tokenStats.input}, output: ${tokenStats.output})`,
    );
    await logDebug(`Total time: ${(elapsedMs / 1000).toFixed(2)}s`);
    await logDebug('Session completed successfully');

    process.exit(0);
  },
};

export default command;

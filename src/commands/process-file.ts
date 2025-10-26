import { readFile } from 'fs/promises';
import OpenAI from 'openai';
import chalk from 'chalk';
import * as path from 'path';
import { parseConfig, SupportedModel } from '../config.js';
import { readConfigFile } from '../config-manager.js';
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

interface ProcessFileArgs {
  input: string;
  output: string;
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
  force: boolean;
  limit?: number;
  log: boolean;
  'very-verbose': boolean;
}

const command: Command<ProcessFileArgs> = {
  command: 'process-file <input>',
  describe: 'Process notes from a file with AI (supports resume)',

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
      .option('force', {
        alias: 'f',
        describe: 'Re-process all rows, ignoring existing output',
        type: 'boolean',
        default: false,
      })
      .option('limit', {
        describe: 'Limit the number of new rows to process (for testing)',
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
      })
      .example(
        '$0 process-file input.csv -o output.csv --field Translation -p prompt.txt',
        'Process a file and update a single field',
      )
      .example(
        '$0 process-file data.yaml -o result.yaml --json -p prompt.txt',
        'Merge JSON response into all fields',
      )
      .example(
        '$0 process-file input.yaml -o output.yaml --field Text -p prompt.txt --limit 10',
        'Test with 10 rows first',
      );
  },

  handler: async (argv) => {
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
      const outputDir = path.dirname(argv.output);
      const outputName = path.basename(argv.output, path.extname(argv.output));
      logFilePath = path.join(outputDir, `${outputName}.log`);
      await initLogger(logFilePath, argv['very-verbose']);
      await logDebug('='.repeat(60));
      await logDebug('File Processing - Session Started');
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
    logInfo(chalk.bold('File-Based Processing'));
    logInfo(chalk.bold('='.repeat(60)));
    logInfo(`Input file:        ${argv.input}`);
    logInfo(`Output file:       ${argv.output}`);
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

    try {
      await logDebug(`Input file: ${argv.input}`);
      await logDebug(`Output file: ${argv.output}`);

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

      // Validate field exists or will be added
      if (!argv.json && rows.length > 0) {
        const fieldExists = argv.field! in rows[0];
        await logDebug(
          fieldExists
            ? `Field "${argv.field}" exists in input data`
            : `Field "${argv.field}" will be added as a new field`,
        );
      }

      // Validate no duplicate noteIds
      await logDebug('Validating no duplicate noteIds in input');
      const seenIds = new Set<string>();
      for (let i = 0; i < rows.length; i++) {
        const noteId = requireNoteId(rows[i]);
        if (seenIds.has(noteId)) {
          throw new AbortError(
            `Duplicate noteId "${noteId}" detected in input (first seen at row ${seenIds.size + 1}, duplicate at row ${i + 1})`,
          );
        }
        seenIds.add(noteId);
      }
      await logDebug('No duplicate noteIds found');

      // Load existing output and filter rows (unless --force)
      const force = argv.force;
      let existingRowsMap = new Map<string, RowData>();
      let rowsToProcess = rows;

      if (!force) {
        await logInfoTee(`\n${chalk.cyan('Loading')} existing output...`);
        existingRowsMap = await loadExistingOutput(argv.output);

        if (existingRowsMap.size > 0) {
          await logInfoTee(
            chalk.green(
              `✓ Found ${existingRowsMap.size} already-processed rows`,
            ),
          );

          // Filter out rows that are already processed (skip rows with errors - retry them)
          rowsToProcess = rows.filter((row) => {
            const noteId = requireNoteId(row);
            const existing = existingRowsMap.get(noteId);
            return !existing || existing._error;
          });

          const skippedCount = rows.length - rowsToProcess.length;
          if (skippedCount > 0) {
            await logInfoTee(
              chalk.yellow(
                `⏭  Skipping ${skippedCount} already-processed rows`,
              ),
            );
          }
        } else {
          await logDebug('No existing output found, will process all rows');
        }
      } else {
        await logDebug('Force mode enabled, will re-process all rows');
      }

      // Apply limit if specified
      if (argv.limit) {
        const originalCount = rowsToProcess.length;
        if (rowsToProcess.length > argv.limit) {
          rowsToProcess = rowsToProcess.slice(0, argv.limit);
          await logInfoTee(
            `\n${chalk.yellow('Limiting')} processing to ${rowsToProcess.length} of ${originalCount} new rows (due to --limit=${argv.limit})`,
          );
        }
      }

      if (rowsToProcess.length === 0) {
        await logInfoTee(
          chalk.green('\n✓ All rows already processed. Nothing to do.'),
        );
        return;
      }

      await logDebug(`Total rows to process: ${rowsToProcess.length}`);

      // Handle dry run
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
        argv.field ?? null,
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
      await logDebug('File processing complete');

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

      process.exit(failures.length > 0 ? 1 : 0);
    } catch (error) {
      if (error instanceof Error) {
        console.log(`\n${chalk.red('✗ Error:')} ${error.message}`);
      } else {
        console.log(`\n${chalk.red('✗ Unknown error:')}`, error);
      }
      console.log('\nMake sure:');
      console.log(`  1. Input file '${argv.input}' exists and is valid`);
      console.log('  2. The file format is correct (CSV or YAML)');
      console.log('  3. All notes have a valid noteId field');
      process.exit(1);
    }
  },
};

export default command;

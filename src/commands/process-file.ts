import chalk from 'chalk';
import * as path from 'path';
import { parseConfig } from '../config.js';
import { logDebug, logInfo, logInfoTee } from '../batch-processing/logger.js';
import {
  parseDataFile,
  loadExistingOutput,
} from '../batch-processing/data-io.js';
import { processAllRows } from '../batch-processing/processor.js';
import { requireNoteId } from '../batch-processing/util.js';
import type { RowData } from '../batch-processing/types.js';
import { AbortError } from 'p-retry';
import type { Command } from './types.js';
import {
  applyCommonProcessingOptions,
  createOpenAIClient,
  finalizeProcessing,
  loadPromptTemplate,
  maybeHandleDryRun,
  printProcessingHeader,
  resolveModelOrExit,
  setupProcessingLogger,
  type CommonProcessingArgs,
} from './process-shared.js';

interface ProcessFileArgs extends CommonProcessingArgs {
  input: string;
  output: string;
  force: boolean;
}

const command: Command<ProcessFileArgs> = {
  command: 'process-file <input>',
  describe: 'Process notes from a file with AI (supports resume)',

  builder: (yargs) => {
    return applyCommonProcessingOptions(
      yargs.positional('input', {
        describe: 'Input file path (CSV or YAML)',
        type: 'string',
        demandOption: true,
      }),
      {
        limitDescription:
          'Limit the number of new rows to process (for testing)',
      },
    )
      .option('output', {
        alias: 'o',
        describe: 'Output file path (CSV or YAML)',
        type: 'string',
        demandOption: true,
      })
      .option('force', {
        alias: 'f',
        describe: 'Re-process all rows, ignoring existing output',
        type: 'boolean',
        default: false,
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

    const model = await resolveModelOrExit(argv.model);

    // Initialize logger
    const logFilePath = await setupProcessingLogger({
      enabled: argv.log || argv['very-verbose'],
      getLogFilePath: () => {
        const outputDir = path.dirname(argv.output);
        const outputName = path.basename(
          argv.output,
          path.extname(argv.output),
        );
        return path.join(outputDir, `${outputName}.log`);
      },
      sessionName: 'File Processing',
      veryVerbose: argv['very-verbose'],
    });

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
    const promptTemplate = await loadPromptTemplate(argv.prompt);

    // Print header
    printProcessingHeader({
      title: 'File-Based Processing',
      extraLines: [
        `Input file:        ${argv.input}`,
        `Output file:       ${argv.output}`,
      ],
      logFilePath,
      jsonMode: argv.json,
      fieldName: argv.field,
      config,
    });

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
      if (
        await maybeHandleDryRun({
          config,
          rows: rowsToProcess,
          promptTemplate,
          itemLabel: 'rows',
          sampleLabel: 'Sample row:',
          dryRunMessage: 'No changes will be saved',
        })
      ) {
        return;
      }

      // Initialize OpenAI client
      await logDebug(`Initializing OpenAI client for model: ${config.model}`);
      const client = createOpenAIClient(config);

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
      const exitCode = await finalizeProcessing({
        processedRows,
        tokenStats,
        config,
        startTime,
      });

      process.exit(exitCode);
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

import { readFile } from 'fs/promises';
import { z } from 'zod';
import OpenAI from 'openai';
import chalk from 'chalk';
import * as path from 'path';
import { parseConfig } from './config.js';
import { parseCliArgs } from './batch-processing/cli.js';
import {
  initLogger,
  logDebug,
  logInfo,
  logInfoTee,
} from './batch-processing/logger.js';
import {
  parseDataFile,
  loadExistingOutput,
} from './batch-processing/data-io.js';
import { processAllRows } from './batch-processing/processor.js';
import { printSummary } from './batch-processing/reporting.js';
import { requireNoteId, fillTemplate } from './batch-processing/util.js';
import type { RowData } from './batch-processing/types.js';
import { AbortError } from 'p-retry';

async function main(): Promise<void> {
  const startTime = Date.now();

  // 1. Parse CLI arguments
  const argv = parseCliArgs();

  // 2. Initialize logger
  const outputDir = path.dirname(argv.output);
  const outputName = path.basename(argv.output, path.extname(argv.output));
  const logFilePath = path.join(outputDir, `${outputName}.log`);
  await initLogger(logFilePath);

  await logDebug('='.repeat(60));
  await logDebug('Batch AI Data Processing - Session Started');
  await logDebug('='.repeat(60));

  // 3. Parse configuration from CLI args
  const config = parseConfig({
    model: argv.model,
    batchSize: argv.batchSize,
    maxTokens: argv.maxTokens,
    temperature: argv.temperature,
    retries: argv.retries,
    dryRun: argv.dryRun,
    requireResultTag: argv.requireResultTag,
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

  // 4. Read prompt template from file
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

  // 5. Read and parse data file
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

  // 6. Validate field exists
  await logDebug('Validating field exists in input data');
  if (rows.length > 0 && !(argv.field in rows[0])) {
    const error = `Field "${argv.field}" not found in input file. Available fields: ${Object.keys(rows[0]).join(', ')}`;
    await logDebug(`ERROR: ${error}`);
    throw new Error(error);
  }
  await logDebug(`Field "${argv.field}" validated successfully`);

  // 7. Validate no duplicate noteIds in input
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

  // 8. Load existing output and filter rows (always enabled unless --force)
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

  // 9. Handle dry run mode
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

  // 10. Initialize OpenAI client
  await logDebug(`Initializing OpenAI client for model: ${config.model}`);
  const client = new OpenAI({
    apiKey: config.apiKey,
    ...(config.apiBaseUrl && { baseURL: config.apiBaseUrl }),
  });

  // 11. Process remaining rows with incremental writing
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

  // 12. Print summary
  const elapsedMs = Date.now() - startTime;
  printSummary(processedRows, tokenStats, config, elapsedMs);

  // 13. Log summary to file
  const failures = processedRows.filter((r) => r._error);
  const successes = processedRows.length - failures.length;
  await logDebug(`Summary: ${successes} successful, ${failures.length} failed`);
  await logDebug(
    `Total tokens: ${tokenStats.input + tokenStats.output} (input: ${tokenStats.input}, output: ${tokenStats.output})`,
  );
  await logDebug(`Total time: ${(elapsedMs / 1000).toFixed(2)}s`);
  await logDebug('Session completed successfully');

  process.exit(0);
}

// Run main and handle errors
main().catch((error) => {
  if (error instanceof Error) {
    console.error(chalk.red(`\n❌ Error: ${error.message}`));
    // Note: logFilePath is now initialized in main(), so we can't access it here
    // Log errors will be handled by the logger module if initialized
    if (error instanceof z.ZodError) {
      console.error(chalk.red('Validation details:'));
      console.error(z.prettifyError(error));
    }
  } else {
    console.error(chalk.red('\n❌ An unknown error occurred:'), error);
  }
  process.exit(1);
});

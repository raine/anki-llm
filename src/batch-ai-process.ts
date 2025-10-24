import { readFile } from 'fs/promises';
import { z } from 'zod';
import OpenAI from 'openai';
import chalk from 'chalk';
import * as path from 'path';
import { parseConfig } from './config.js';
import { parseCliArgs } from './batch-processing/cli.js';
import { initLogger, log } from './batch-processing/logger.js';
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

  await log('='.repeat(60));
  await log('Batch AI Data Processing - Session Started');
  await log('='.repeat(60));

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

  await log(`Log file: ${logFilePath}`);
  await log(`Input file: ${argv.input}`);
  await log(`Output file: ${argv.output}`);
  await log(`Field to process: ${argv.field}`);
  await log(`Model: ${config.model}`);
  await log(`Batch size: ${config.batchSize}`);
  await log(`Retries: ${config.retries}`);
  await log(`Temperature: ${config.temperature}`);
  if (config.maxTokens) {
    await log(`Max tokens: ${config.maxTokens}`);
  }
  await log(`Dry run: ${config.dryRun}`);
  await log(`Require result tag: ${config.requireResultTag}`);

  // 4. Read prompt template from file
  const promptTemplate = await readFile(argv.prompt, 'utf-8');
  await log(`Prompt template loaded (${promptTemplate.length} chars)`);

  console.log(chalk.bold('='.repeat(60)));
  console.log(chalk.bold('Batch AI Data Processing'));
  console.log(chalk.bold('='.repeat(60)));
  console.log(`Input file:        ${argv.input}`);
  console.log(`Output file:       ${argv.output}`);
  console.log(`Log file:          ${logFilePath}`);
  console.log(`Field to process:  ${argv.field}`);
  console.log(`Model:             ${config.model}`);
  console.log(`Batch size:        ${config.batchSize}`);
  console.log(`Retries:           ${config.retries}`);
  console.log(`Temperature:       ${config.temperature}`);
  if (config.maxTokens) {
    console.log(`Max tokens:        ${config.maxTokens}`);
  }
  console.log(`Dry run:           ${config.dryRun}`);
  console.log(`Require result tag: ${config.requireResultTag}`);
  console.log(chalk.bold('='.repeat(60)));

  // 5. Read and parse data file
  console.log(`\n${chalk.cyan('Reading')} ${argv.input}...`);
  await log(`Reading input file: ${argv.input}`);
  const rows = await parseDataFile(argv.input);
  const inputFormat = path.extname(argv.input).substring(1).toUpperCase();
  console.log(chalk.green(`✓ Found ${rows.length} rows in ${inputFormat}`));
  await log(`Parsed ${rows.length} rows from ${inputFormat} file`);

  if (rows.length === 0) {
    console.log(chalk.yellow('No rows to process. Exiting.'));
    await log('No rows found. Exiting.');
    return;
  }

  // 6. Validate field exists
  await log('Validating field exists in input data');
  if (rows.length > 0 && !(argv.field in rows[0])) {
    const error = `Field "${argv.field}" not found in input file. Available fields: ${Object.keys(rows[0]).join(', ')}`;
    await log(`ERROR: ${error}`);
    throw new Error(error);
  }
  await log(`Field "${argv.field}" validated successfully`);

  // 7. Validate no duplicate noteIds in input
  await log('Validating no duplicate noteIds in input');
  const seenIds = new Set<string>();
  for (let i = 0; i < rows.length; i++) {
    const noteId = requireNoteId(rows[i]);
    if (seenIds.has(noteId)) {
      const error = `Duplicate noteId "${noteId}" detected in input (first seen at row ${seenIds.size + 1}, duplicate at row ${i + 1})`;
      await log(`ERROR: ${error}`);
      throw new AbortError(error);
    }
    seenIds.add(noteId);
  }
  await log('No duplicate noteIds found');

  // 8. Load existing output and filter rows (always enabled unless --force)
  const force = argv.force;

  let existingRowsMap = new Map<string, RowData>();
  let rowsToProcess = rows;

  if (!force) {
    console.log(`\n${chalk.cyan('Loading')} existing output...`);
    await log('Loading existing output to skip already-processed rows');
    existingRowsMap = await loadExistingOutput(argv.output);

    if (existingRowsMap.size > 0) {
      console.log(
        chalk.green(`✓ Found ${existingRowsMap.size} already-processed rows`),
      );
      await log(`Found ${existingRowsMap.size} already-processed rows`);

      // Filter out rows that are already processed (skip rows with errors - retry them)
      rowsToProcess = rows.filter((row) => {
        const noteId = requireNoteId(row);
        const existing = existingRowsMap.get(noteId);
        // Process if: no existing row, or existing row has an error
        return !existing || existing._error;
      });

      const skippedCount = rows.length - rowsToProcess.length;
      if (skippedCount > 0) {
        console.log(
          chalk.yellow(`⏭  Skipping ${skippedCount} already-processed rows`),
        );
        await log(`Skipping ${skippedCount} already-processed rows`);
      }
    } else {
      await log('No existing output found, will process all rows');
    }
  } else {
    await log('Force mode enabled, will re-process all rows');
  }

  if (rowsToProcess.length === 0) {
    console.log(chalk.green('\n✓ All rows already processed. Nothing to do.'));
    await log('All rows already processed. Exiting.');
    return;
  }

  await log(`Total rows to process: ${rowsToProcess.length}`);

  // 9. Handle dry run mode
  if (config.dryRun) {
    console.log(chalk.yellow('\n⚠️  DRY RUN MODE - No changes will be saved'));
    console.log(`Would process ${rowsToProcess.length} rows`);
    console.log(`\n${chalk.bold('Prompt template:')}`);
    console.log(promptTemplate);
    console.log(`\n${chalk.bold('Sample row:')}`);
    console.log(JSON.stringify(rowsToProcess[0], null, 2));
    console.log(`\n${chalk.bold('Sample prompt:')}`);
    console.log(fillTemplate(promptTemplate, rowsToProcess[0]));
    await log('Dry run complete. Exiting.');
    return;
  }

  // 10. Initialize OpenAI client
  await log(`Initializing OpenAI client for model: ${config.model}`);
  const client = new OpenAI({
    apiKey: config.apiKey,
    ...(config.apiBaseUrl && { baseURL: config.apiBaseUrl }),
  });

  // 11. Process remaining rows with incremental writing
  console.log(`\n${chalk.cyan('Processing')} ${rowsToProcess.length} rows...`);
  await log(`Starting batch processing of ${rowsToProcess.length} rows`);
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

  console.log(chalk.green(`\n✓ Processing complete`));
  await log('Batch processing complete');

  // 12. Print summary
  const elapsedMs = Date.now() - startTime;
  printSummary(processedRows, tokenStats, config, elapsedMs);

  // 13. Log summary to file
  const failures = processedRows.filter((r) => r._error);
  const successes = processedRows.length - failures.length;
  await log(`Summary: ${successes} successful, ${failures.length} failed`);
  await log(
    `Total tokens: ${tokenStats.input + tokenStats.output} (input: ${tokenStats.input}, output: ${tokenStats.output})`,
  );
  await log(`Total time: ${(elapsedMs / 1000).toFixed(2)}s`);
  await log('Session completed successfully');
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

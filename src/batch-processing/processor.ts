import OpenAI from 'openai';
import pRetry, { AbortError } from 'p-retry';
import pLimit from 'p-limit';
import cliProgress from 'cli-progress';
import chalk from 'chalk';
import type { Config } from '../config.js';
import type { RowData, ProcessedRow, TokenStats } from './types.js';
import { requireNoteId } from './util.js';
import { log } from './logger.js';
import { processSingleRow } from './llm.service.js';
import { serializeData, atomicWriteFile } from './data-io.js';
import { calculateCost } from './reporting.js';

/**
 * Process rows with concurrency control and retry logic without batch-blocking.
 */
export async function processAllRows(
  rows: RowData[],
  fieldToProcess: string,
  promptTemplate: string,
  config: Config,
  client: OpenAI,
  options?: {
    allRows?: RowData[];
    existingRowsMap?: Map<string, RowData>;
    outputPath?: string;
  },
): Promise<{
  rows: ProcessedRow[];
  tokenStats: TokenStats;
}> {
  const limit = pLimit(config.batchSize);
  const tokenStats: TokenStats = { input: 0, output: 0 };

  const progressBar = new cliProgress.SingleBar({
    format:
      'Processing |' +
      chalk.cyan('{bar}') +
      '| {percentage}% | {value}/{total} rows | ETA: {eta}s',
    barCompleteChar: '\u2588',
    barIncompleteChar: '\u2591',
    hideCursor: true,
  });

  progressBar.start(rows.length, 0);

  // This array will store results in the original order.
  const orderedResults: ProcessedRow[] = new Array<ProcessedRow>(rows.length);

  // --- Logic for buffered incremental writing ---
  const processedMap = new Map<string, RowData>();
  const completedBuffer: ProcessedRow[] = [];
  let completedCount = 0; // Track how many rows have completed

  const performIncrementalWrite = async () => {
    if (!options?.outputPath || !options?.allRows || !options?.existingRowsMap)
      return;

    // Update the central map with the buffered results
    for (const row of completedBuffer) {
      processedMap.set(requireNoteId(row), row);
    }
    completedBuffer.length = 0; // Clear buffer

    // Merge new results with existing ones to create the full output file.
    // Note: This preserves the existing behavior of only writing rows that
    // have a processed result (from this run or a previous one).
    const finalRows: RowData[] = [];
    for (const row of options.allRows) {
      const noteId = requireNoteId(row);
      const processedRow =
        processedMap.get(noteId) || options.existingRowsMap.get(noteId);
      if (processedRow) {
        finalRows.push(processedRow);
      }
    }

    // Write to file atomically
    const outputContent = serializeData(finalRows, options.outputPath);
    await atomicWriteFile(options.outputPath, outputContent);
  };
  // --- End of incremental writing logic ---

  const allPromises = rows.map((row, index) =>
    limit(async () => {
      let result: ProcessedRow;
      try {
        const processedValue = await pRetry(
          () =>
            processSingleRow({
              row,
              promptTemplate,
              config,
              client,
              tokenStats,
            }),
          {
            retries: config.retries,
            onFailedAttempt: async (error) => {
              const rowId = requireNoteId(row);
              const errorMsg =
                error instanceof Error ? error.message : 'Unknown error';
              const retryMsg = `Retry ${error.attemptNumber}/${config.retries + 1} for row ${rowId}: ${errorMsg}`;
              console.log(chalk.yellow(`\n  ${retryMsg}`));
              await log(retryMsg, true);
            },
            minTimeout: 1000,
            maxTimeout: 30000,
            factor: 2,
          },
        );
        result = { ...row, [fieldToProcess]: processedValue };
      } catch (error) {
        if (error instanceof AbortError) {
          throw error; // Critical error, stop everything
        }
        const errorMessage =
          error instanceof Error ? error.message : 'Unknown error';
        const rowId = requireNoteId(row);
        await log(
          `Row ${rowId}: FAILED after all retries - ${errorMessage}`,
          true,
        );
        result = { ...row, _error: errorMessage };
      }

      // Store result in order and handle incremental writing
      orderedResults[index] = result;
      completedCount++;

      // Log cost every 10 completed rows
      if (completedCount % 10 === 0) {
        const currentCost = calculateCost(tokenStats, config.model);
        await log(
          `Progress: ${completedCount} rows completed | Tokens: ${tokenStats.input + tokenStats.output} (in: ${tokenStats.input}, out: ${tokenStats.output}) | Cost so far: $${currentCost.toFixed(4)}`,
          true,
        );
      }

      if (options?.outputPath) {
        completedBuffer.push(result);
        if (completedBuffer.length >= config.batchSize) {
          await performIncrementalWrite();
        }
      }

      progressBar.increment();
    }),
  );

  // Wait for all queued promises to complete
  await Promise.all(allPromises);

  // Write any remaining items in the buffer
  if (completedBuffer.length > 0) {
    await performIncrementalWrite();
  }

  progressBar.stop();

  return { rows: orderedResults, tokenStats };
}

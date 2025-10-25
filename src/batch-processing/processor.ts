import OpenAI from 'openai';
import pRetry, { AbortError } from 'p-retry';
import pLimit from 'p-limit';
import cliProgress from 'cli-progress';
import chalk from 'chalk';
import type { Config } from '../config.js';
import type { RowData, ProcessedRow, TokenStats } from './types.js';
import {
  requireNoteId,
  tryParseJsonObject,
  mergeFieldsCaseInsensitive,
} from './util.js';
import { logDebug } from './logger.js';
import { processSingleRow } from './llm.js';
import { serializeData, atomicWriteFile } from './data-io.js';
import { calculateCost } from './reporting.js';

/**
 * Process rows with concurrency control and retry logic without batch-blocking.
 */
export async function processAllRows(
  rows: RowData[],
  fieldToProcess: string | null, // null = JSON mode, string = single field mode
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
      '| {percentage}% | {value}/{total} rows | Cost: ${cost} | Tokens: {tokens}',
    barCompleteChar: '\u2588',
    barIncompleteChar: '\u2591',
    hideCursor: true,
  });

  progressBar.start(rows.length, 0, {
    cost: '0.0000',
    tokens: '0',
  });

  // This array will store results in the original order.
  const orderedResults: ProcessedRow[] = new Array<ProcessedRow>(rows.length);

  // --- Logic for buffered incremental writing ---
  const processedMap = new Map<string, RowData>();
  const completedBuffer: ProcessedRow[] = [];

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
      const rowId = requireNoteId(row); // Get rowId early for logging
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
              const errorMsg =
                error instanceof Error ? error.message : 'Unknown error';
              const retryMsg = `Retry ${error.attemptNumber}/${config.retries + 1} for row ${rowId}: ${errorMsg}`;
              console.log(chalk.yellow(`\n  ${retryMsg}`));
              await logDebug(retryMsg);
            },
            minTimeout: 1000,
            maxTimeout: 30000,
            factor: 2,
          },
        );

        // Handle response based on mode
        if (fieldToProcess === null) {
          // JSON mode: expect and require JSON response
          const parsedJson = tryParseJsonObject(processedValue);
          if (parsedJson) {
            // Perform case-insensitive merge to prevent field duplication
            result = mergeFieldsCaseInsensitive(row, parsedJson);
            await logDebug(
              `Row ${rowId}: Merged JSON response into note. Fields updated: ${Object.keys(parsedJson).join(', ')}`,
            );
          } else {
            // JSON mode but response is not valid JSON - this is an error
            throw new Error(
              `Expected JSON response in --json mode, but received: ${processedValue.substring(0, 100)}...`,
            );
          }
        } else {
          // Single field mode: update the specified field
          result = { ...row, [fieldToProcess]: processedValue };
          await logDebug(
            `Row ${rowId}: Updated field '${fieldToProcess}' with response.`,
          );
        }
      } catch (error) {
        if (error instanceof AbortError) {
          throw error; // Critical error, stop everything
        }
        const errorMessage =
          error instanceof Error ? error.message : 'Unknown error';
        await logDebug(
          `Row ${rowId}: FAILED after all retries - ${errorMessage}`,
        );
        result = { ...row, _error: errorMessage };
      }

      // Store result in order and handle incremental writing
      orderedResults[index] = result;

      if (options?.outputPath) {
        completedBuffer.push(result);
        if (completedBuffer.length >= config.batchSize) {
          await performIncrementalWrite();
        }
      }

      // Calculate stats and update progress bar with real-time cost/token info
      const totalTokens = tokenStats.input + tokenStats.output;
      const currentCost = calculateCost(tokenStats, config.model);
      progressBar.increment(1, {
        cost: currentCost.toFixed(4),
        tokens: totalTokens.toString(),
      });
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

import OpenAI from 'openai';
import pRetry, { AbortError } from 'p-retry';
import pLimit from 'p-limit';
import cliProgress from 'cli-progress';
import chalk from 'chalk';
import type { Config } from '../config.js';
import type { RowData, TokenStats } from './types.js';
import {
  requireNoteId,
  tryParseJsonObject,
  mergeFieldsCaseInsensitive,
} from './util.js';
import { logDebug } from './logger.js';
import { processSingleRow } from './llm.js';
import { calculateCost } from './reporting.js';

/**
 * Hooks to manage the lifecycle of the processing task.
 * This allows the core processing logic to be generic, while the
 * I/O and state management can be specific to the use case.
 */
export interface ProcessorHooks {
  /** Called when a row is successfully processed. */
  onSuccess: (
    processedRow: RowData,
    originalRow: RowData,
    index: number,
  ) => Promise<void>;
  /** Called when a row fails processing after all retries. */
  onFailure: (
    originalRow: RowData,
    error: Error,
    index: number,
  ) => Promise<void>;
  /** Called after each row is processed, useful for batching operations. */
  onBatchProcessed?: () => Promise<void>;
  /** Called after all rows have been processed. */
  onCompletion: () => Promise<void>;
}

/**
 * A generic processor that handles concurrency, retries, progress, and cleanup.
 * It uses a hooks-based approach to delegate I/O and state management.
 */
export async function runProcessor(
  rows: RowData[],
  fieldToProcess: string | null,
  promptTemplate: string,
  config: Config,
  client: OpenAI,
  hooks: ProcessorHooks,
  tokenStats: TokenStats, // Mutated directly
): Promise<void> {
  const limit = pLimit(config.batchSize);

  const progressBar = new cliProgress.SingleBar({
    format:
      'Processing |' +
      chalk.cyan('{bar}') +
      '| {percentage}% | {value}/{total} items | Cost: ${cost} | Tokens: {tokens}',
    barCompleteChar: '\u2588',
    barIncompleteChar: '\u2591',
    hideCursor: true,
  });

  const cleanupProgressBar = () => {
    progressBar.stop();
    process.stdout.write('\x1B[?25h'); // Show cursor
  };

  const signalHandler = () => {
    cleanupProgressBar();
    process.exit(130);
  };

  process.on('SIGINT', signalHandler);
  process.on('SIGTERM', signalHandler);

  progressBar.start(rows.length, 0, { cost: '0.0000', tokens: '0' });

  const allPromises = rows.map((row, index) =>
    limit(async () => {
      const rowId = requireNoteId(row);
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
              const retryMsg = `Retry ${error.attemptNumber}/${config.retries + 1} for item ${rowId}: ${errorMsg}`;
              console.log(chalk.yellow(`\n  ${retryMsg}`));
              await logDebug(retryMsg);
            },
            minTimeout: 1000,
            maxTimeout: 30000,
            factor: 2,
          },
        );

        let processedRow: RowData;
        if (fieldToProcess === null) {
          const parsedJson = tryParseJsonObject(processedValue);
          if (parsedJson) {
            processedRow = mergeFieldsCaseInsensitive(row, parsedJson);
            await logDebug(
              `Item ${rowId}: Merged JSON response. Fields updated: ${Object.keys(parsedJson).join(', ')}`,
            );
          } else {
            throw new Error(
              `Expected JSON response, but received: ${processedValue.substring(0, 100)}...`,
            );
          }
        } else {
          processedRow = { ...row, [fieldToProcess]: processedValue };
          await logDebug(`Item ${rowId}: Updated field '${fieldToProcess}'.`);
        }

        await hooks.onSuccess(processedRow, row, index);
      } catch (error) {
        if (error instanceof AbortError) throw error;
        const typedError =
          error instanceof Error ? error : new Error(String(error));
        await logDebug(
          `Item ${rowId}: FAILED after all retries - ${typedError.message}`,
        );
        await hooks.onFailure(row, typedError, index);
      }

      const totalTokens = tokenStats.input + tokenStats.output;
      const currentCost = calculateCost(tokenStats, config.model);
      progressBar.increment(1, {
        cost: currentCost.toFixed(4),
        tokens: totalTokens.toString(),
      });

      if (hooks.onBatchProcessed) {
        await hooks.onBatchProcessed();
      }
    }),
  );

  try {
    await Promise.all(allPromises);
    await hooks.onCompletion();
  } finally {
    cleanupProgressBar();
    process.removeListener('SIGINT', signalHandler);
    process.removeListener('SIGTERM', signalHandler);
  }
}

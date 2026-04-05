import OpenAI from 'openai';
import type { Config } from '../config.js';
import type { RowData, ProcessedRow, TokenStats } from './types.js';
import { requireNoteId } from './util.js';
import { serializeData, atomicWriteFile } from './data-io.js';
import { runProcessor } from './core-processor.js';

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
  const tokenStats: TokenStats = { input: 0, output: 0 };
  const orderedResults: ProcessedRow[] = new Array<ProcessedRow>(rows.length);
  const completedBuffer: ProcessedRow[] = [];
  const processedMap = new Map<string, RowData>();

  const performIncrementalWrite = async () => {
    if (!options?.outputPath || !options?.allRows || !options?.existingRowsMap)
      return;

    // Update the central map with the buffered results
    for (const row of completedBuffer) {
      processedMap.set(requireNoteId(row), row);
    }
    completedBuffer.length = 0; // Clear buffer

    // Merge new results with existing ones to create the full output file.
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

  await runProcessor(
    rows,
    fieldToProcess,
    promptTemplate,
    config,
    client,
    {
      onSuccess: (processedRow, _originalRow, index) => {
        orderedResults[index] = processedRow;
        if (options?.outputPath) {
          completedBuffer.push(processedRow);
        }
        return Promise.resolve();
      },
      onFailure: (originalRow, error, index) => {
        const result: ProcessedRow = { ...originalRow, _error: error.message };
        orderedResults[index] = result;
        if (options?.outputPath) {
          completedBuffer.push(result);
        }
        return Promise.resolve();
      },
      onBatchProcessed: async () => {
        if (options?.outputPath && completedBuffer.length >= config.batchSize) {
          await performIncrementalWrite();
        }
      },
      onCompletion: async () => {
        if (options?.outputPath && completedBuffer.length > 0) {
          await performIncrementalWrite();
        }
      },
    },
    tokenStats,
  );

  return { rows: orderedResults, tokenStats };
}

import OpenAI from 'openai';
import { writeFile } from 'fs/promises';
import type { Config } from '../config.js';
import type { RowData, TokenStats } from './types.js';
import { requireNoteId } from './util.js';
import { logDebug } from './logger.js';
import { runProcessor } from './core-processor.js';

/**
 * Represents a note that failed to process
 */
interface FailedNote {
  error: string;
  note: RowData;
}

/**
 * Batch update notes in Anki using the multi action for efficiency
 */
async function batchUpdateAnkiNotes(
  updateQueue: Array<{ id: number; fields: Record<string, string> }>,
  ankiRequest: (
    action: string,
    params: Record<string, unknown>,
  ) => Promise<unknown>,
): Promise<void> {
  if (updateQueue.length === 0) return;

  await logDebug(
    `Batch updating ${updateQueue.length} notes in Anki via multi action`,
  );

  const updateActions = updateQueue.map((note) => ({
    action: 'updateNoteFields',
    params: { note },
  }));

  const results = (await ankiRequest('multi', {
    actions: updateActions,
  })) as unknown[];

  // Check for failures (successful updateNoteFields returns null)
  const failures = results.filter((r) => r !== null);
  if (failures.length > 0) {
    const errorMsg = `${failures.length} Anki update operations failed`;
    await logDebug(`ERROR: ${errorMsg}. Details: ${JSON.stringify(failures)}`);
    throw new Error(errorMsg);
  }

  await logDebug(`Successfully updated ${updateQueue.length} notes in Anki`);
}

/**
 * Process notes directly from Anki deck and update them in real-time.
 * Unlike processAllRows (file-based), this:
 * - Takes notes from Anki instead of a file
 * - Updates Anki directly instead of writing to a file
 * - Does not support resume (no incremental file writes)
 * - Logs failures to a separate error file
 */
export async function processDirect(
  notes: RowData[],
  fieldToProcess: string | null, // null = JSON mode, string = single field mode
  promptTemplate: string,
  config: Config,
  client: OpenAI,
  ankiRequest: (
    action: string,
    params: Record<string, unknown>,
  ) => Promise<unknown>,
  errorLogPath: string,
): Promise<{
  successes: number;
  failures: FailedNote[];
  tokenStats: TokenStats;
}> {
  const tokenStats: TokenStats = { input: 0, output: 0 };
  const failedNotes: FailedNote[] = [];
  const updateQueue: Array<{ id: number; fields: Record<string, string> }> = [];

  const flushUpdateQueue = async () => {
    if (updateQueue.length === 0) return;
    try {
      await batchUpdateAnkiNotes(updateQueue, ankiRequest);
      updateQueue.length = 0; // Clear queue
    } catch (error) {
      await logDebug(
        `ERROR: Failed to batch update Anki notes: ${error instanceof Error ? error.message : String(error)}`,
      );
      throw error; // Propagate to stop processing
    }
  };

  await runProcessor(
    notes,
    fieldToProcess,
    promptTemplate,
    config,
    client,
    {
      onSuccess: (processedNote) => {
        const noteId = requireNoteId(processedNote);
        const fieldsToUpdate: Record<string, string> = {};
        for (const [key, value] of Object.entries(processedNote)) {
          if (
            key !== 'noteId' &&
            key !== 'id' &&
            key !== 'Id' &&
            !key.startsWith('_')
          ) {
            fieldsToUpdate[key] = String(value ?? '');
          }
        }
        updateQueue.push({ id: Number(noteId), fields: fieldsToUpdate });
        return Promise.resolve();
      },
      onFailure: (note, error) => {
        failedNotes.push({ error: error.message, note });
        return Promise.resolve();
      },
      onBatchProcessed: async () => {
        if (updateQueue.length >= config.batchSize) {
          await flushUpdateQueue();
        }
      },
      onCompletion: async () => {
        await flushUpdateQueue(); // Final flush
        if (failedNotes.length > 0) {
          const errorLogContent = failedNotes
            .map((f) => JSON.stringify(f))
            .join('\n');
          await writeFile(errorLogPath, errorLogContent, 'utf-8');
          await logDebug(
            `Wrote ${failedNotes.length} failed notes to ${errorLogPath}`,
          );
        }
      },
    },
    tokenStats,
  );

  return {
    successes: notes.length - failedNotes.length,
    failures: failedNotes,
    tokenStats,
  };
}

import OpenAI from 'openai';
import chalk from 'chalk';
import { z } from 'zod';
import { ankiRequest, ankiRequestRaw, NoteInfo } from '../anki-connect.js';
import { logDebug, logInfo, logInfoTee } from '../batch-processing/logger.js';
import { processDirect } from '../batch-processing/direct-processor.js';
import { printSummary } from '../batch-processing/reporting.js';
import { requireNoteId, slugifyDeckName } from '../batch-processing/util.js';
import type { RowData, ProcessedRow } from '../batch-processing/types.js';
import { AbortError } from 'p-retry';
import {
  commonProcessingHandler,
  SharedProcessingArgs,
} from './shared-processing.js';
import { parseDataFile, loadExistingOutput } from './data-io.js';
import { processAllRows } from './processor.js';
import * as path from 'path';

interface ProcessDeckArgs extends SharedProcessingArgs {
  deck: string;
}

interface ProcessFileArgs extends SharedProcessingArgs {
  input: string;
  output: string;
  force: boolean;
}

type NoteInfoType = z.infer<typeof NoteInfo>;

export async function handleProcessDeck(argv: ProcessDeckArgs) {
  try {
    const deckName = argv.deck;
    await logDebug(`Mode: Direct processing for deck "${deckName}"`);

    // Fetch notes from Anki
    await logInfoTee(`\n${chalk.cyan('Fetching')} notes from deck...`);
    const query = `deck:"${deckName}"`;
    const noteIds = await ankiRequest('findNotes', z.array(z.number()), {
      query,
    });

    if (noteIds.length === 0) {
      await logInfoTee(
        chalk.yellow(`No notes found in deck '${deckName}'. Exiting.`),
      );
      return;
    }

    await logInfoTee(chalk.green(`✓ Found ${noteIds.length} notes`));

    // Fetch detailed note information
    await logInfoTee(`${chalk.cyan('Loading')} note details...`);
    const notesInfo = await ankiRequest('notesInfo', z.array(NoteInfo), {
      notes: noteIds,
    });

    // Convert notes to RowData format
    const rows: RowData[] = notesInfo.map((note: NoteInfoType) => {
      const row: RowData = { noteId: note.noteId };
      for (const [fieldName, fieldData] of Object.entries(note.fields)) {
        if (fieldData) {
          row[fieldName] = fieldData.value.replace(/\r/g, '');
        }
      }
      return row;
    });

    await logInfoTee(chalk.green(`✓ Loaded ${rows.length} notes`));

    // Validate no duplicate noteIds
    await logDebug('Validating no duplicate noteIds');
    const seenIds = new Set<string>();
    for (let i = 0; i < rows.length; i++) {
      const noteId = requireNoteId(rows[i]);
      if (seenIds.has(noteId)) {
        throw new AbortError(
          `Duplicate noteId "${noteId}" detected (row ${i + 1})`,
        );
      }
      seenIds.add(noteId);
    }

    // Apply limit if specified
    let notesToProcess = rows;
    if (argv.limit) {
      if (notesToProcess.length > argv.limit) {
        notesToProcess = notesToProcess.slice(0, argv.limit);
        await logInfoTee(
          `\n${chalk.yellow('Limiting')} processing to ${
            notesToProcess.length
          } of ${rows.length} notes (due to --limit=${argv.limit})`,
        );
      }
    }

    const commonResult = await commonProcessingHandler(
      argv,
      slugifyDeckName(argv.deck),
      notesToProcess,
    );

    if (!commonResult) {
      return; // Dry run or other pre-exit condition
    }

    const { config, promptTemplate, startTime } = commonResult;

    // Initialize OpenAI client
    await logDebug(`Initializing OpenAI client for model: ${config.model}`);
    const client = new OpenAI({
      apiKey: config.apiKey,
      ...(config.apiBaseUrl && { baseURL: config.apiBaseUrl }),
    });

    // Process notes and update Anki directly
    const errorLogPath = `${slugifyDeckName(deckName)}-errors.jsonl`;
    await logInfoTee(
      `\n${chalk.cyan('Processing')} ${notesToProcess.length} notes...`,
    );

    const { successes, failures, tokenStats } = await processDirect(
      notesToProcess,
      argv.field ?? null,
      promptTemplate,
      config,
      client,
      ankiRequestRaw,
      errorLogPath,
    );

    logInfo(chalk.green(`\n✓ Processing complete`));
    await logDebug('Direct processing complete');

    // Create ProcessedRow array for summary (matching file mode structure)
    const processedRows: ProcessedRow[] = notesToProcess.map((note) => {
      const failure = failures.find(
        (f) => requireNoteId(f.note) === requireNoteId(note),
      );
      return failure ? { ...note, _error: failure.error } : note;
    });

    // Print summary
    const elapsedMs = Date.now() - startTime;
    printSummary(processedRows, tokenStats, config, elapsedMs, {
      errorLogPath: failures.length > 0 ? errorLogPath : undefined,
    });

    // Log summary to file
    await logDebug(
      `Summary: ${successes} successful, ${failures.length} failed`,
    );
    await logDebug(
      `Total tokens: ${tokenStats.input + tokenStats.output} (input: ${
        tokenStats.input
      }, output: ${tokenStats.output})`,
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
    console.log('  1. Anki Desktop is running');
    console.log('  2. AnkiConnect add-on is installed (code: 2055492159)');
    console.log(`  3. Deck '${argv.deck}' exists`);
    process.exit(1);
  }
}

export async function handleProcessFile(argv: ProcessFileArgs) {
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
          `Duplicate noteId "${noteId}" detected in input (first seen at row ${
            seenIds.size + 1
          }, duplicate at row ${i + 1})`,
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
          chalk.green(`✓ Found ${existingRowsMap.size} already-processed rows`),
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
            chalk.yellow(`⏭  Skipping ${skippedCount} already-processed rows`),
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
          `\n${chalk.yellow('Limiting')} processing to ${
            rowsToProcess.length
          } of ${originalCount} new rows (due to --limit=${argv.limit})`,
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

    const outputName = path.basename(argv.output, path.extname(argv.output));
    const commonResult = await commonProcessingHandler(
      argv,
      outputName,
      rowsToProcess,
    );

    if (!commonResult) {
      return; // Dry run or other pre-exit condition
    }

    const { config, promptTemplate, startTime } = commonResult;

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
      `Total tokens: ${tokenStats.input + tokenStats.output} (input: ${
        tokenStats.input
      }, output: ${tokenStats.output})`,
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
}

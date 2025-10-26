import chalk from 'chalk';
import { z } from 'zod';
import { parseConfig } from '../config.js';
import { ankiRequest, ankiRequestRaw, NoteInfo } from '../anki-connect.js';
import { logDebug, logInfo, logInfoTee } from '../batch-processing/logger.js';
import { processDirect } from '../batch-processing/direct-processor.js';
import { requireNoteId } from '../batch-processing/util.js';
import type { RowData, ProcessedRow } from '../batch-processing/types.js';
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

type NoteInfoType = z.infer<typeof NoteInfo>;

interface ProcessDeckArgs extends CommonProcessingArgs {
  deck: string;
  limit?: number;
}

/**
 * Slugifies a deck name for use in filenames
 */
function slugifyDeckName(deckName: string): string {
  const parts = deckName.split('::');
  const lastPart = parts[parts.length - 1];
  return lastPart
    .toString()
    .toLowerCase()
    .trim()
    .replace(/[^a-z0-9\s-]/g, '')
    .replace(/[\s-]+/g, '-')
    .replace(/^-+|-+$/g, '');
}

const command: Command<ProcessDeckArgs> = {
  command: 'process-deck <deck>',
  describe: 'Process notes directly from an Anki deck (no intermediate files)',

  builder: (yargs) => {
    return applyCommonProcessingOptions(
      yargs.positional('deck', {
        describe: 'Name of the Anki deck to process',
        type: 'string',
        demandOption: true,
      }),
      {
        limitDescription: 'Limit the number of notes to process (for testing)',
      },
    )
      .example(
        '$0 process-deck "Japanese Core 1k" --field Translation -p prompt.txt',
        'Process a deck and update a single field',
      )
      .example(
        '$0 process-deck "Vocabulary" --json -p prompt.txt',
        'Merge JSON response into all fields',
      )
      .example(
        '$0 process-deck "My Deck" --field Notes -p prompt.txt --limit 10',
        'Test with 10 notes first',
      );
  },

  handler: async (argv) => {
    const startTime = Date.now();

    const model = await resolveModelOrExit(argv.model);

    // Initialize logger
    const logFilePath = await setupProcessingLogger({
      enabled: argv.log || argv['very-verbose'],
      getLogFilePath: () => {
        const deckSlug = slugifyDeckName(argv.deck);
        return `${deckSlug}-process.log`;
      },
      sessionName: 'Direct Processing',
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
      title: 'Direct Anki Processing',
      extraLines: [`Deck:              ${argv.deck}`],
      logFilePath,
      jsonMode: argv.json,
      fieldName: argv.field,
      config,
    });

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
            `\n${chalk.yellow('Limiting')} processing to ${notesToProcess.length} of ${rows.length} notes (due to --limit=${argv.limit})`,
          );
        }
      }

      // Handle dry run
      if (
        await maybeHandleDryRun({
          config,
          rows: notesToProcess,
          promptTemplate,
          itemLabel: 'notes',
          sampleLabel: 'Sample note:',
          dryRunMessage: 'No changes will be made',
        })
      ) {
        return;
      }

      // Initialize OpenAI client
      await logDebug(`Initializing OpenAI client for model: ${config.model}`);
      const client = createOpenAIClient(config);

      // Process notes and update Anki directly
      const errorLogPath = `${slugifyDeckName(deckName)}-errors.jsonl`;
      await logInfoTee(
        `\n${chalk.cyan('Processing')} ${notesToProcess.length} notes...`,
      );

      const { failures, tokenStats } = await processDirect(
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
      const exitCode = await finalizeProcessing({
        processedRows,
        tokenStats,
        config,
        startTime,
        errorLogPath: failures.length > 0 ? errorLogPath : undefined,
      });
      process.exit(exitCode);
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
  },
};

export default command;

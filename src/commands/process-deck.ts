import { readFile } from 'fs/promises';
import OpenAI from 'openai';
import chalk from 'chalk';
import { z } from 'zod';
import { parseConfig, SupportedModel } from '../config.js';
import { ankiRequest, ankiRequestRaw, NoteInfo } from '../anki-connect.js';
import {
  initLogger,
  logDebug,
  logInfo,
  logInfoTee,
} from '../batch-processing/logger.js';
import { processDirect } from '../batch-processing/direct-processor.js';
import { printSummary } from '../batch-processing/reporting.js';
import { requireNoteId, fillTemplate } from '../batch-processing/util.js';
import type { RowData, ProcessedRow } from '../batch-processing/types.js';
import { AbortError } from 'p-retry';
import type { Command } from './types.js';

type NoteInfoType = z.infer<typeof NoteInfo>;

interface ProcessDeckArgs {
  deck: string;
  field?: string;
  json: boolean;
  prompt: string;
  model: string;
  'batch-size': number;
  'max-tokens'?: number;
  temperature: number;
  retries: number;
  'dry-run': boolean;
  'require-result-tag': boolean;
  limit?: number;
  log: boolean;
  'very-verbose': boolean;
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
    return yargs
      .positional('deck', {
        describe: 'Name of the Anki deck to process',
        type: 'string',
        demandOption: true,
      })
      .option('field', {
        describe:
          'Field name to update with AI response (mutually exclusive with --json)',
        type: 'string',
      })
      .option('json', {
        describe:
          'Expect JSON response and merge all fields (mutually exclusive with --field)',
        type: 'boolean',
        default: false,
      })
      .option('prompt', {
        alias: 'p',
        describe: 'Path to prompt template file',
        type: 'string',
        demandOption: true,
      })
      .option('model', {
        alias: 'm',
        describe: `Model to use. Available: ${SupportedModel.options.join(', ')}`,
        type: 'string',
        default: 'gpt-4o-mini',
      })
      .option('batch-size', {
        alias: 'b',
        describe: 'Number of concurrent API requests',
        type: 'number',
        default: 5,
      })
      .option('max-tokens', {
        describe: 'Maximum tokens for completion',
        type: 'number',
      })
      .option('temperature', {
        alias: 't',
        describe: 'Temperature for model sampling',
        type: 'number',
        default: 0.0,
      })
      .option('retries', {
        alias: 'r',
        describe: 'Number of retries for failed requests',
        type: 'number',
        default: 3,
      })
      .option('dry-run', {
        alias: 'd',
        describe: 'Preview operation without making API calls',
        type: 'boolean',
        default: false,
      })
      .option('require-result-tag', {
        describe: 'Require <result> tags in AI responses',
        type: 'boolean',
        default: false,
      })
      .option('limit', {
        describe: 'Limit the number of notes to process (for testing)',
        type: 'number',
      })
      .option('log', {
        describe: 'Generate a log file',
        type: 'boolean',
        default: false,
      })
      .option('very-verbose', {
        describe: 'Log LLM responses to log file (automatically enables --log)',
        type: 'boolean',
        default: false,
      })
      .check((argv) => {
        if (argv.limit !== undefined && argv.limit <= 0) {
          throw new Error('Error: --limit must be a positive number.');
        }
        // Require either --field or --json (but not both)
        if (!argv.field && !argv.json) {
          throw new Error('Error: Either --field or --json must be specified.');
        }
        if (argv.field && argv.json) {
          throw new Error(
            'Error: --field and --json are mutually exclusive. Use only one.',
          );
        }
        return true;
      })
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

    // Initialize logger
    let logFilePath: string | null = null;
    if (argv.log || argv['very-verbose']) {
      const deckSlug = slugifyDeckName(argv.deck);
      logFilePath = `${deckSlug}-process.log`;
      await initLogger(logFilePath, argv['very-verbose']);
      await logDebug('='.repeat(60));
      await logDebug('Direct Processing - Session Started');
      await logDebug('='.repeat(60));
      if (argv['very-verbose']) {
        await logDebug('Very verbose mode enabled - will log LLM responses');
      }
    }

    // Parse configuration
    const config = parseConfig({
      model: argv.model,
      batchSize: argv['batch-size'],
      maxTokens: argv['max-tokens'],
      temperature: argv.temperature,
      retries: argv.retries,
      dryRun: argv['dry-run'],
      requireResultTag: argv['require-result-tag'],
    });

    // Read prompt template
    const promptTemplate = await readFile(argv.prompt, 'utf-8');
    await logDebug(`Prompt template loaded (${promptTemplate.length} chars)`);

    // Print header
    logInfo(chalk.bold('='.repeat(60)));
    logInfo(chalk.bold('Direct Anki Processing'));
    logInfo(chalk.bold('='.repeat(60)));
    logInfo(`Deck:              ${argv.deck}`);
    if (logFilePath) {
      logInfo(`Log file:          ${logFilePath}`);
    }
    if (argv.json) {
      logInfo(`Mode:              JSON merge`);
    } else {
      logInfo(`Field to process:  ${argv.field}`);
    }
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
      if (config.dryRun) {
        logInfo(chalk.yellow('\n⚠️  DRY RUN MODE - No changes will be made'));
        logInfo(`Would process ${notesToProcess.length} notes`);
        logInfo(`\n${chalk.bold('Prompt template:')}`);
        logInfo(promptTemplate);
        logInfo(`\n${chalk.bold('Sample note:')}`);
        logInfo(JSON.stringify(notesToProcess[0], null, 2));
        logInfo(`\n${chalk.bold('Sample prompt:')}`);
        logInfo(fillTemplate(promptTemplate, notesToProcess[0]));
        await logDebug('Dry run complete. Exiting.');
        return;
      }

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
        `Total tokens: ${tokenStats.input + tokenStats.output} (input: ${tokenStats.input}, output: ${tokenStats.output})`,
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
  },
};

export default command;

import { readFile } from 'fs/promises';
import Papa from 'papaparse';
import { z } from 'zod';
import { ankiRequest } from '../anki-connect.js';
import * as path from 'path';
import * as yaml from 'js-yaml';
import { findModelNameForDeck, getFieldNamesForModel } from '../anki-schema.js';
import type { Command } from './types.js';

const NoteFieldValue = z.object({ value: z.string(), order: z.number() });

const AnkiNoteInfo = z.object({
  noteId: z.number(),
  modelName: z.string(),
  fields: z.record(z.string(), NoteFieldValue.optional()),
});
type AnkiNoteInfo = z.infer<typeof AnkiNoteInfo>;

const AnkiNote = z.object({
  deckName: z.string(),
  modelName: z.string(),
  fields: z.record(z.string(), z.string()),
  tags: z.array(z.string()),
});
type AnkiNote = z.infer<typeof AnkiNote>;

interface ImportDeckArgs {
  input: string;
  deck: string;
  'note-type'?: string;
  'key-field'?: string;
}

const command: Command<ImportDeckArgs> = {
  command: 'import <input>',
  describe: 'Import CSV or YAML file into an Anki deck',

  builder: (yargs) => {
    return yargs
      .positional('input', {
        describe: 'Input file path (CSV or YAML)',
        type: 'string',
        demandOption: true,
      })
      .option('deck', {
        alias: 'd',
        describe: 'Target Anki deck name',
        type: 'string',
        demandOption: true,
      })
      .option('note-type', {
        alias: 'n',
        describe: 'Anki note type name (inferred from deck if not specified)',
        type: 'string',
        demandOption: false,
      })
      .option('key-field', {
        alias: 'k',
        describe:
          'Field name to use for identifying existing notes (auto-detected if not specified)',
        type: 'string',
      })
      .example(
        '$0 import export.csv --deck "My Deck" --note-type "Basic" --key-field noteId',
        'Import using noteId as key',
      )
      .example(
        '$0 import data.yaml -d "Japanese" -n "Custom" -k Id',
        'Import using Id as key',
      );
  },

  handler: async (argv) => {
    console.log('='.repeat(60));
    console.log(`Importing from ${argv.input} to deck: ${argv.deck}`);

    try {
      // Infer note type if not specified
      let modelName = argv['note-type'];
      if (!modelName) {
        console.log(`\nNote type not specified, inferring from deck...`);
        const inferredModel = await findModelNameForDeck(argv.deck);
        if (!inferredModel) {
          throw new Error(
            `Could not infer note type from deck '${argv.deck}'. The deck may be empty or not exist. Please specify the note type explicitly using --note-type.`,
          );
        }
        modelName = inferredModel;
        console.log(`✓ Inferred note type: ${modelName}`);
      }

      console.log(`Note type: ${modelName}`);
      console.log('='.repeat(60));

      // Read and Parse file (CSV or YAML)
      console.log(`\nReading and parsing ${argv.input}...`);
      const rows = await parseDataFile(argv.input);
      console.log(`✓ Found ${rows.length} rows in ${argv.input}.`);

      if (rows.length === 0) {
        console.log('No rows to import. Exiting.');
        return;
      }

      // Get note type fields (used for auto-detection and validation)
      console.log(`\nValidating fields against note type '${modelName}'...`);
      const modelFields = await getFieldNamesForModel(modelName);
      console.log(`✓ Note type fields: ${modelFields.join(', ')}`);

      // Auto-detect key field if not specified
      let keyField = argv['key-field'];
      const inputFileFields = Object.keys(rows[0]);

      if (!keyField) {
        console.log('\nAuto-detecting key field...');
        if (inputFileFields.includes('noteId')) {
          keyField = 'noteId';
          console.log(`✓ Found 'noteId' column. Using as key field.`);
        } else {
          // Use first field of note type as fallback
          const firstModelField = modelFields[0];
          if (firstModelField && inputFileFields.includes(firstModelField)) {
            keyField = firstModelField;
            console.log(
              `✓ Using first field of note type '${modelName}' as key: ${keyField}`,
            );
          } else {
            throw new Error(
              `Could not auto-detect key field. 'noteId' column not found, and the note type's first field ('${firstModelField}') is not in the input file. Please specify the key field manually with --key-field.`,
            );
          }
        }
      }

      console.log(`Key field: ${keyField}`);
      console.log('='.repeat(60));

      // Validate that the key field exists in the parsed data
      if (!(keyField in rows[0])) {
        throw new Error(
          `Key field "${keyField}" not found in input file. Available fields: ${inputFileFields.join(', ')}`,
        );
      }

      // Check which fields from the file are valid for the note type
      const fileFields = Object.keys(rows[0]).filter((f) => f !== keyField);
      const invalidFields = fileFields.filter((f) => !modelFields.includes(f));

      if (invalidFields.length > 0) {
        console.warn(
          `\n⚠️  Warning: The following fields in the input file do not exist in the note type and will be ignored:`,
        );
        console.warn(`  ${invalidFields.join(', ')}`);
      }

      const validFields = fileFields.filter((f) => modelFields.includes(f));
      console.log(`✓ Valid fields to import: ${validFields.join(', ')}`);

      // Fetch existing notes from the deck to create a key -> noteId map
      console.log(`\nFetching existing notes from deck '${argv.deck}'...`);
      const existingNoteIds = await ankiRequest(
        'findNotes',
        z.array(z.number()),
        {
          query: `deck:"${argv.deck}"`,
        },
      );

      const keyToNoteIdMap = new Map<string, number>();

      if (existingNoteIds.length > 0) {
        const notesInfo = await ankiRequest(
          'notesInfo',
          z.array(AnkiNoteInfo),
          {
            notes: existingNoteIds,
          },
        );

        for (const note of notesInfo) {
          let keyValue: string | undefined;

          // Special handling for noteId as key
          if (keyField === 'noteId') {
            keyValue = String(note.noteId);
          } else {
            const fieldValue = note.fields[keyField]?.value;
            if (fieldValue) {
              keyValue = fieldValue;
            }
          }

          if (keyValue) {
            keyToNoteIdMap.set(keyValue, note.noteId);
          }
        }
      }
      console.log(
        `✓ Found ${keyToNoteIdMap.size} existing notes with a '${keyField}' field.`,
      );

      // Partition rows into notes to add and notes to update
      console.log('\nPartitioning notes for insert or update...');
      const notesToAdd: AnkiNote[] = [];
      const notesToUpdate: { id: number; fields: Record<string, string> }[] =
        [];

      for (const row of rows) {
        // Extract only valid fields for the note type
        const fields: Record<string, string> = {};
        for (const field of validFields) {
          fields[field] = String(row[field] ?? '');
        }

        const keyValue = String(row[keyField]);
        const existingNoteId = keyToNoteIdMap.get(keyValue);

        if (existingNoteId) {
          notesToUpdate.push({
            id: existingNoteId,
            fields,
          });
        } else {
          // For new notes, include the key field if it's not noteId
          if (keyField !== 'noteId') {
            fields[keyField] = keyValue;
          }

          notesToAdd.push({
            deckName: argv.deck,
            modelName: modelName,
            fields,
            tags: ['anki-llm-import'],
          });
        }
      }

      console.log(`✓ Partitioning complete:`);
      console.log(`  - ${notesToAdd.length} new notes to add.`);
      console.log(`  - ${notesToUpdate.length} existing notes to update.`);

      // Add new notes (if any)
      if (notesToAdd.length > 0) {
        console.log(`\nAdding ${notesToAdd.length} new notes...`);
        const addResult = await ankiRequest(
          'addNotes',
          z.array(z.number().nullable()),
          { notes: notesToAdd },
        );
        const successes = addResult.filter((r) => r !== null).length;
        const failures = addResult.length - successes;
        console.log(
          `✓ Add operation complete: ${successes} succeeded, ${failures} failed.`,
        );
        if (failures > 0) {
          console.warn(
            '  - Some notes failed to add. Check Anki for possible reasons.',
          );
        }
      }

      // Update existing notes (if any)
      if (notesToUpdate.length > 0) {
        console.log(`\nUpdating ${notesToUpdate.length} existing notes...`);
        const updateActions = notesToUpdate.map((note) => ({
          action: 'updateNoteFields',
          params: { note },
        }));

        const updateResult = await ankiRequest('multi', z.array(z.unknown()), {
          actions: updateActions,
        });

        // The result of a 'multi' call is an array of results for each action.
        // A successful 'updateNoteFields' action returns null.
        const failures = updateResult.filter((r) => r !== null);
        if (failures.length > 0) {
          console.error(
            `✗ Update operation failed for ${failures.length} notes.`,
          );
          console.error('  - Failure details:', failures);
        } else {
          console.log(
            `✓ Update operation complete: ${notesToUpdate.length} notes updated successfully.`,
          );
        }
      }

      console.log('\nImport process finished.');
    } catch (error) {
      if (error instanceof Error) {
        console.error(`\n✗ Fatal Error: ${error.message}`);
      } else {
        console.error('\n✗ An unknown fatal error occurred:', error);
      }
      console.log('\nMake sure:');
      console.log('  1. Anki Desktop is running with AnkiConnect installed.');
      console.log(`  2. The deck '${argv.deck}' exists.`);
      console.log(
        `  3. The note type exists and its fields match the input file.`,
      );
      process.exit(1);
    }
  },
};

export default command;

// Helper functions

/**
 * Parses a data file (CSV or YAML) into an array of row objects.
 */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
async function parseDataFile(filePath: string): Promise<Record<string, any>[]> {
  const fileContent = await readFile(filePath, 'utf-8');
  const ext = path.extname(filePath).toLowerCase();

  if (ext === '.yaml' || ext === '.yml') {
    const parsedData = yaml.load(fileContent);
    if (!Array.isArray(parsedData)) {
      throw new Error('YAML content is not an array');
    }
    // eslint-disable-next-line @typescript-eslint/no-unsafe-return
    return parsedData;
  } else if (ext === '.csv') {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const parseResult = Papa.parse<Record<string, any>>(fileContent, {
      header: true,
      skipEmptyLines: true,
    });

    if (parseResult.errors.length > 0) {
      throw new Error(
        `CSV parsing errors: ${JSON.stringify(parseResult.errors)}`,
      );
    }
    return parseResult.data;
  } else {
    throw new Error(
      `Unsupported file format: ${ext}. Please use .csv, .yaml, or .yml extension.`,
    );
  }
}

import { readFile } from 'fs/promises';
import Papa from 'papaparse';
import { z } from 'zod';
import { ankiRequest } from './anki-connect.js';

// Parse command-line arguments
const args = process.argv.slice(2);
const INPUT_FILE = args[0] || 'glossika_deck_export.csv';
const DECK_NAME = args[1] || 'Glossika-ENJA [2001-3000]';
const MODEL_NAME = args[2] || 'Glossika'; // IMPORTANT: Must match your Anki model name

// Zod schemas
const CsvRow = z.object({
  id: z.string(),
  english: z.string(),
  japanese: z.string(),
  ka: z.string(),
  ROM: z.string(),
  explanation: z.string(),
});
type CsvRow = z.infer<typeof CsvRow>;

const AnkiNote = z.object({
  deckName: z.string(),
  modelName: z.string(),
  fields: z.record(z.string(), z.string()),
  tags: z.array(z.string()),
});
type AnkiNote = z.infer<typeof AnkiNote>;

const NoteFieldValue = z.object({ value: z.string(), order: z.number() });

const AnkiNoteInfo = z.object({
  noteId: z.number(),
  fields: z.record(z.string(), NoteFieldValue.optional()),
});
type AnkiNoteInfo = z.infer<typeof AnkiNoteInfo>;

async function importCsvToAnki(): Promise<void> {
  console.log('='.repeat(60));
  console.log(`Importing from ${INPUT_FILE} to deck: ${DECK_NAME}`);
  console.log('='.repeat(60));

  try {
    // Read and Parse CSV
    console.log(`\nReading and parsing ${INPUT_FILE}...`);
    const fileContent = await readFile(INPUT_FILE, 'utf-8');
    const parseResult = Papa.parse<CsvRow>(fileContent, {
      header: true,
      skipEmptyLines: true,
    });

    if (parseResult.errors.length > 0) {
      throw new Error(
        `CSV parsing errors: ${JSON.stringify(parseResult.errors)}`,
      );
    }
    const rows = parseResult.data;
    console.log(`✓ Found ${rows.length} rows in CSV.`);

    if (rows.length === 0) {
      console.log('No rows to import. Exiting.');
      return;
    }

    // Fetch existing notes from the deck to create an ID -> noteId map
    console.log(`\nFetching existing notes from deck '${DECK_NAME}'...`);
    const existingNoteIds = await ankiRequest(
      'findNotes',
      z.array(z.number()),
      {
        query: `deck:"${DECK_NAME}"`,
      },
    );

    const idToNoteIdMap = new Map<string, number>();

    if (existingNoteIds.length > 0) {
      const notesInfo = await ankiRequest('notesInfo', z.array(AnkiNoteInfo), {
        notes: existingNoteIds,
      });

      for (const note of notesInfo) {
        if (note.fields.Id?.value) {
          idToNoteIdMap.set(note.fields.Id.value, note.noteId);
        }
      }
    }
    console.log(
      `✓ Found ${idToNoteIdMap.size} existing notes with an 'Id' field.`,
    );

    // Partition CSV rows into notes to add and notes to update
    console.log('\nPartitioning notes for insert or update...');
    const notesToAdd: AnkiNote[] = [];
    const notesToUpdate: { id: number; fields: Record<string, string> }[] = [];

    const allNotesFromCsv = rows.map((row) => CsvRow.parse(row));

    for (const row of allNotesFromCsv) {
      const fields = {
        Id: row.id,
        English: row.english,
        Japanese: row.japanese,
        か: row.ka,
        ROM: row.ROM,
        Explanation: row.explanation,
      };

      const existingNoteId = idToNoteIdMap.get(row.id);

      if (existingNoteId) {
        notesToUpdate.push({
          id: existingNoteId,
          fields,
        });
      } else {
        notesToAdd.push({
          deckName: DECK_NAME,
          modelName: MODEL_NAME,
          fields,
          tags: ['glossika-import-script'],
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
    console.log(`  2. The deck '${DECK_NAME}' exists.`);
    console.log(
      `  3. The model '${MODEL_NAME}' exists and its fields match the script.`,
    );
  }
}

void importCsvToAnki();

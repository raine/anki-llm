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

    // Transform CSV rows to Anki note format
    const notesToImport: AnkiNote[] = rows.map((row) => ({
      deckName: DECK_NAME,
      modelName: MODEL_NAME,
      fields: {
        Id: row.id,
        English: row.english,
        Japanese: row.japanese,
        か: row.ka,
        ROM: row.ROM,
        Explanation: row.explanation,
      },
      tags: ['glossika-import-script'],
    }));

    // Pre-flight check for duplicates
    console.log('\nPerforming pre-flight check for duplicates...');
    const canAddResult = await ankiRequest(
      'canAddNotes',
      z.array(z.boolean()),
      {
        notes: notesToImport,
      },
    );

    const notesToAdd: AnkiNote[] = [];
    let duplicateCount = 0;
    canAddResult.forEach((canAdd: boolean, index: number) => {
      if (canAdd) {
        notesToAdd.push(notesToImport[index]);
      } else {
        duplicateCount++;
        console.warn(
          `- Skipping duplicate (row ${index + 2}): ${notesToImport[index].fields['English']}`,
        );
      }
    });

    if (duplicateCount > 0) {
      console.log(
        `✓ Pre-flight complete. Found and skipped ${duplicateCount} duplicates.`,
      );
    } else {
      console.log('✓ Pre-flight complete. No duplicates found.');
    }

    if (notesToAdd.length === 0) {
      console.log('\nNo new notes to import. Exiting.');
      return;
    }

    // Add Notes to Anki
    console.log(`\nImporting ${notesToAdd.length} new notes...`);
    const addNotesResult = await ankiRequest(
      'addNotes',
      z.array(z.number().nullable()),
      {
        notes: notesToAdd,
      },
    );

    let successCount = 0;
    let failureCount = 0;
    addNotesResult.forEach((result: number | null, index: number) => {
      if (result === null) {
        failureCount++;
        console.error(
          `✗ Failed to import row for note: ${notesToAdd[index].fields['English']}`,
        );
      } else {
        successCount++;
      }
    });

    console.log(`\n✓ Import finished.`);
    console.log(`  - Successful: ${successCount}`);
    console.log(`  - Failed:     ${failureCount}`);
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

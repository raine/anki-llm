import { writeFile } from 'fs/promises';
import { z } from 'zod';
import { ankiRequest, NoteInfo } from './anki-connect.js';
import * as path from 'path';
import * as yaml from 'js-yaml';
import Papa from 'papaparse';

// Parse command-line arguments
const args = process.argv.slice(2);
const DECK_NAME = args[0] || 'Glossika-ENJA [2001-3000]';
const OUTPUT_FILE = args[1] || 'glossika_deck_export.csv';

// Type inference from Zod schemas
type NoteInfo = z.infer<typeof NoteInfo>;
type CsvRow = {
  id: string;
  english: string;
  japanese: string;
  ka: string;
  ROM: string;
  explanation: string;
};

/**
 * Converts an array of rows to the appropriate format based on file extension.
 */
function serializeRows(rows: CsvRow[], filePath: string): string {
  const ext = path.extname(filePath).toLowerCase();

  if (ext === '.yaml' || ext === '.yml') {
    // Use js-yaml with lineWidth: -1 to prevent line wrapping
    return yaml.dump(rows, { lineWidth: -1 });
  } else if (ext === '.csv') {
    // Use papaparse for robust CSV generation
    return Papa.unparse(rows);
  } else {
    throw new Error(
      `Unsupported file format: ${ext}. Please use .csv, .yaml, or .yml extension.`,
    );
  }
}

/**
 * Helper function to safely extract and clean field values from a note.
 */
function getFieldValue(fields: NoteInfo['fields'], fieldName: string): string {
  const rawValue = fields[fieldName]?.value ?? '';
  return rawValue.replace(/\r/g, '');
}

async function exportDeckToCsv(): Promise<void> {
  console.log('='.repeat(60));
  console.log(`Exporting deck: ${DECK_NAME}`);
  console.log('='.repeat(60));

  try {
    // Find all notes in the deck
    const query = `deck:"${DECK_NAME}"`;
    const noteIds = await ankiRequest('findNotes', z.array(z.number()), {
      query,
    });
    console.log(`\n✓ Found ${noteIds.length} notes in '${DECK_NAME}'.`);

    if (noteIds.length === 0) {
      console.log('No notes found to export.');
      return;
    }

    // Get detailed info for all notes
    console.log(`\nFetching note details...`);
    const notesInfo = await ankiRequest('notesInfo', z.array(NoteInfo), {
      notes: noteIds,
    });
    console.log(`✓ Retrieved information for ${notesInfo.length} notes.`);

    // Convert notes to CSV rows
    const rows: CsvRow[] = notesInfo.map((note: NoteInfo) => {
      return {
        id: getFieldValue(note.fields, 'Id'),
        english: getFieldValue(note.fields, 'English'),
        japanese: getFieldValue(note.fields, 'Japanese'),
        ka: getFieldValue(note.fields, 'か'),
        ROM: getFieldValue(note.fields, 'ROM'),
        explanation: getFieldValue(note.fields, 'Explanation'),
      };
    });

    // Write to file (CSV or YAML)
    console.log(`\nWriting to ${OUTPUT_FILE}...`);
    const content = serializeRows(rows, OUTPUT_FILE);

    await writeFile(OUTPUT_FILE, content, 'utf-8');
    console.log(
      `✓ Successfully exported ${notesInfo.length} notes to ${OUTPUT_FILE}`,
    );
  } catch (error) {
    if (error instanceof Error) {
      console.log(`\n✗ Error: ${error.message}`);
      if (error instanceof z.ZodError) {
        console.log('Validation details:', z.flattenError(error));
      }
    } else {
      console.log('\n✗ An unknown error occurred:', error);
    }
    console.log('\nMake sure:');
    console.log('  1. Anki Desktop is running');
    console.log('  2. AnkiConnect add-on is installed (code: 2055492159)');
    console.log(`  3. Deck '${DECK_NAME}' exists`);
  }
}

void exportDeckToCsv();

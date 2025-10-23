import { writeFile } from 'fs/promises';
import { z } from 'zod';
import { ankiRequest, NoteInfo } from './anki-connect.js';
import * as path from 'path';
import * as yaml from 'js-yaml';
import Papa from 'papaparse';
import { findModelNameForDeck, getFieldNamesForModel } from './anki-schema.js';
import yargs from 'yargs';
import { hideBin } from 'yargs/helpers';

// Parse command-line arguments
const argv = yargs(hideBin(process.argv))
  .usage('Usage: $0 <deck> <output>')
  .command('$0 <deck> <output>', 'Export an Anki deck to CSV or YAML')
  .positional('deck', {
    describe: 'Name of the Anki deck to export',
    type: 'string',
    demandOption: true,
  })
  .positional('output', {
    describe: 'Output file path (CSV or YAML)',
    type: 'string',
    demandOption: true,
  })
  .example('$0 "My Deck" output.csv', 'Export deck to CSV')
  .example('$0 "Japanese" data.yaml', 'Export deck to YAML')
  .help()
  .parseSync();

// Type inference from Zod schemas
type NoteInfo = z.infer<typeof NoteInfo>;

/**
 * Converts an array of rows to the appropriate format based on file extension.
 */
function serializeRows(
  rows: Record<string, string | number>[],
  filePath: string,
): string {
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

async function exportDeck(): Promise<void> {
  console.log('='.repeat(60));
  console.log(`Exporting deck: ${argv.deck}`);
  console.log('='.repeat(60));

  try {
    // Find all notes in the deck
    const query = `deck:"${argv.deck}"`;
    const noteIds = await ankiRequest('findNotes', z.array(z.number()), {
      query,
    });
    console.log(`\n✓ Found ${noteIds.length} notes in '${argv.deck}'.`);

    if (noteIds.length === 0) {
      console.log('No notes found to export.');
      return;
    }

    // Discover model and fields dynamically
    console.log(`\nDiscovering model type and fields...`);
    const modelName = await findModelNameForDeck(argv.deck);
    if (!modelName) {
      throw new Error(`Could not determine model name for deck: ${argv.deck}`);
    }
    console.log(`✓ Model type: ${modelName}`);

    const fieldNames = await getFieldNamesForModel(modelName);
    console.log(`✓ Fields: ${fieldNames.join(', ')}`);

    // Get detailed info for all notes
    console.log(`\nFetching note details...`);
    const notesInfo = await ankiRequest('notesInfo', z.array(NoteInfo), {
      notes: noteIds,
    });
    console.log(`✓ Retrieved information for ${notesInfo.length} notes.`);

    // Convert notes to rows dynamically
    const rows = notesInfo.map((note: NoteInfo) => {
      const row: Record<string, string | number> = {
        noteId: note.noteId, // Export noteId for potential updates
      };
      for (const field of fieldNames) {
        row[field] = getFieldValue(note.fields, field);
      }
      return row;
    });

    // Write to file (CSV or YAML)
    console.log(`\nWriting to ${argv.output}...`);
    const content = serializeRows(rows, argv.output);

    await writeFile(argv.output, content, 'utf-8');
    console.log(
      `✓ Successfully exported ${notesInfo.length} notes to ${argv.output}`,
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
    console.log(`  3. Deck '${argv.deck}' exists`);
  }
}

void exportDeck();

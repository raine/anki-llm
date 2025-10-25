import { writeFile } from 'fs/promises';
import { z } from 'zod';
import { ankiRequest, NoteInfo } from '../anki-connect.js';
import * as path from 'path';
import * as yaml from 'js-yaml';
import Papa from 'papaparse';
import { findModelNameForDeck, getFieldNamesForModel } from '../anki-schema.js';
import type { Command } from './types.js';

// Type inference from Zod schemas
type NoteInfoType = z.infer<typeof NoteInfo>;

interface ExportDeckArgs {
  deck: string;
  output?: string;
}

const command: Command<ExportDeckArgs> = {
  command: 'export <deck> [output]',
  describe: 'Export an Anki deck to CSV or YAML',

  builder: (yargs) => {
    return yargs
      .positional('deck', {
        describe: 'Name of the Anki deck to export',
        type: 'string',
        demandOption: true,
      })
      .positional('output', {
        describe:
          'Output file path. If omitted, generates from deck name (e.g., "My Deck" -> my-deck.yaml)',
        type: 'string',
        demandOption: false,
      })
      .example('$0 export "My Deck"', 'Export deck to my-deck.yaml')
      .example('$0 export "My Deck" .csv', 'Export deck to my-deck.csv')
      .example('$0 export "Japanese" data.yaml', 'Export deck to YAML');
  },

  handler: async (argv) => {
    console.log('='.repeat(60));
    console.log(`Exporting deck: ${argv.deck}`);
    console.log('='.repeat(60));

    try {
      // Resolve output path
      const outputPath = resolveOutputPath(argv.deck, argv.output);

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
        throw new Error(
          `Could not determine model name for deck: ${argv.deck}`,
        );
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
      const rows = notesInfo.map((note: NoteInfoType) => {
        const row: Record<string, string | number> = {
          noteId: note.noteId, // Export noteId for potential updates
        };
        for (const field of fieldNames) {
          row[field] = getFieldValue(note.fields, field);
        }
        return row;
      });

      // Write to file (CSV or YAML)
      console.log(`\nWriting to ${outputPath}...`);
      const content = serializeRows(rows, outputPath);

      await writeFile(outputPath, content, 'utf-8');
      console.log(
        `✓ Successfully exported ${notesInfo.length} notes to ${outputPath}`,
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
      process.exit(1);
    }
  },
};

export default command;

// Helper functions

/**
 * Resolves the output file path based on the deck name and optional output argument.
 * Supports three modes:
 * 1. No output specified -> auto-generate with default .yaml extension
 * 2. Extension only (e.g., ".csv") -> auto-generate filename with specified extension
 * 3. Full path specified -> use as-is
 */
function resolveOutputPath(deckName: string, output?: string): string {
  const defaultExt = '.yaml';

  if (!output) {
    // Case 1: No output specified. Slugify deck name + default extension.
    const outputPath = slugify(deckName) + defaultExt;
    console.log(
      `\n✓ Output file not specified, automatically using '${outputPath}'`,
    );
    return outputPath;
  } else if (output.startsWith('.')) {
    // Case 2: Only extension specified. Slugify deck name + given extension.
    const extension = output;
    if (extension === '.' || !['.yaml', '.yml', '.csv'].includes(extension)) {
      throw new Error(
        `Unsupported file extension: '${extension}'. Please use .csv, .yaml, or .yml.`,
      );
    }
    const outputPath = slugify(deckName) + extension;
    console.log(`\n✓ Automatically generating filename: '${outputPath}'`);
    return outputPath;
  } else {
    // Case 3: Full path specified.
    return output;
  }
}

/**
 * Converts a deck name to a safe filename slug.
 * Handles Anki's :: sub-deck separator by using the last part.
 */
function slugify(text: string): string {
  // Use the last part of the deck name for the filename
  const parts = text.split('::');
  const lastPart = parts[parts.length - 1];

  return (
    lastPart
      .toString()
      .toLowerCase()
      .trim()
      // Replace characters that are not letters, numbers, or hyphens
      .replace(/[^a-z0-9\s-]/g, '')
      // Replace spaces and multiple hyphens with a single hyphen
      .replace(/[\s-]+/g, '-')
      // Remove leading or trailing hyphens
      .replace(/^-+|-+$/g, '')
  );
}

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
function getFieldValue(
  fields: NoteInfoType['fields'],
  fieldName: string,
): string {
  const rawValue = fields[fieldName]?.value ?? '';
  return rawValue.replace(/\r/g, '');
}

import { z } from 'zod';
import { readFile } from 'fs/promises';
import { fileURLToPath } from 'url';
import { dirname, join } from 'path';
import { ankiRequest } from '../anki-connect.js';
import type { Command } from './types.js';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

interface QueryArgs {
  action: string;
  params?: string;
}

const command: Command<QueryArgs> = {
  command: 'query <action> [params]',
  describe: 'Query AnkiConnect API with any action and optional parameters',

  builder: (yargs) => {
    return yargs
      .positional('action', {
        describe: 'AnkiConnect action to perform',
        type: 'string',
        demandOption: true,
      })
      .positional('params', {
        describe: 'JSON string of parameters for the action',
        type: 'string',
      })
      .example('$0 query deckNames', 'Get all deck names')
      .example(
        '$0 query findNotes \'{"query":"deck:Japanese"}\'',
        'Find notes in Japanese deck',
      )
      .example(
        '$0 query cardsInfo \'{"cards":[1498938915662]}\'',
        'Get detailed info about specific cards',
      )
      .example('$0 query modelNames', 'Get all model (note type) names')
      .example(
        '$0 query getDeckStats \'{"decks":["Default"]}\'',
        'Get statistics for the Default deck',
      )
      .example(
        '$0 query docs',
        'Get full AnkiConnect API documentation (useful for AI agents)',
      );
  },

  handler: async (argv) => {
    try {
      // Special case: return AnkiConnect documentation
      if (argv.action === 'docs' || argv.action === 'help') {
        const docPath = join(__dirname, '../../ANKI_CONNECT.md');
        const docs = await readFile(docPath, 'utf-8');
        console.log(docs);
        return;
      }

      // Parse params if provided
      let params: Record<string, unknown> | undefined;
      if (argv.params) {
        try {
          // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
          params = JSON.parse(argv.params);
        } catch (error) {
          console.log(`✗ Error: Invalid JSON in params argument`);
          if (error instanceof Error) {
            console.log(`  ${error.message}`);
          }
          console.log(
            `\nParams must be valid JSON. Example: '{"query":"deck:Default"}'`,
          );
          process.exit(1);
        }
      }

      // Make the request - use z.unknown() for dynamic result validation
      const result = await ankiRequest(argv.action, z.unknown(), params);

      // Pretty print the result as JSON
      console.log(JSON.stringify(result, null, 2));
    } catch (error) {
      if (error instanceof Error) {
        console.log(`✗ Error: ${error.message}`);
      } else {
        console.log('✗ An unknown error occurred:', error);
      }
      console.log('\nMake sure:');
      console.log('  1. Anki Desktop is running');
      console.log('  2. AnkiConnect add-on is installed (code: 2055492159)');
      console.log(`  3. The action '${argv.action}' is valid`);
      console.log('  4. The params are correctly formatted for this action\n');
      console.log(
        'See ANKI_CONNECT.md for documentation on available actions and their parameters.',
      );
      process.exit(1);
    }
  },
};

export default command;

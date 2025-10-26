import type { Command } from './types.js';
import { addSharedOptions } from '../batch-processing/processing-options.js';
import { SharedProcessingArgs } from '../batch-processing/shared-processing.js';
import { handleProcessDeck } from '../batch-processing/command-handler.js';

interface ProcessDeckArgs extends SharedProcessingArgs {
  deck: string;
}

const command: Command<ProcessDeckArgs> = {
  command: 'process-deck <deck>',
  describe: 'Process notes directly from an Anki deck (no intermediate files)',

  builder: (yargs) => {
    return addSharedOptions(yargs)
      .positional('deck', {
        describe: 'Name of the Anki deck to process',
        type: 'string',
        demandOption: true,
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
    await handleProcessDeck(argv);
  },
};

export default command;

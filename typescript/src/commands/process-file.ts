import type { Command } from './types.js';
import { addSharedOptions } from '../batch-processing/processing-options.js';
import { SharedProcessingArgs } from '../batch-processing/shared-processing.js';
import { handleProcessFile } from '../batch-processing/command-handler.js';

interface ProcessFileArgs extends SharedProcessingArgs {
  input: string;
  output: string;
  force: boolean;
}

const command: Command<ProcessFileArgs> = {
  command: 'process-file <input>',
  describe: 'Process notes from a file with AI (supports resume)',

  builder: (yargs) => {
    return addSharedOptions(yargs)
      .positional('input', {
        describe: 'Input file path (CSV or YAML)',
        type: 'string',
        demandOption: true,
      })
      .option('output', {
        alias: 'o',
        describe: 'Output file path (CSV or YAML)',
        type: 'string',
        demandOption: true,
      })
      .option('force', {
        alias: 'f',
        describe: 'Re-process all rows, ignoring existing output',
        type: 'boolean',
        default: false,
      })
      .example(
        '$0 process-file input.csv -o output.csv --field Translation -p prompt.txt',
        'Process a file and update a single field',
      )
      .example(
        '$0 process-file data.yaml -o result.yaml --json -p prompt.txt',
        'Merge JSON response into all fields',
      )
      .example(
        '$0 process-file input.yaml -o output.yaml --field Text -p prompt.txt --limit 10',
        'Test with 10 rows first',
      );
  },

  handler: async (argv) => {
    await handleProcessFile(argv);
  },
};

export default command;

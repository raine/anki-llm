#!/usr/bin/env node
import yargs from 'yargs';
import { hideBin } from 'yargs/helpers';

// Import command modules
import exportCmd from './commands/export.js';
import importCmd from './commands/import.js';
import batchCmd from './commands/batch.js';
import queryCmd from './commands/query.js';

void yargs(hideBin(process.argv))
  // Register commands
  .command(exportCmd)
  .command(importCmd)
  .command(batchCmd)
  .command(queryCmd)
  // Configuration
  .scriptName('anki-llm-batch')
  .demandCommand(1, 'You must provide a valid command.')
  .strict()
  .help()
  .alias('h', 'help')
  .version()
  .alias('v', 'version')
  .parse();

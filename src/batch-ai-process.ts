import { readFile, writeFile } from 'fs/promises';
import Papa from 'papaparse';
import { z } from 'zod';
import OpenAI from 'openai';
import pRetry, { AbortError } from 'p-retry';
import pLimit from 'p-limit';
import cliProgress from 'cli-progress';
import chalk from 'chalk';
import * as path from 'path';
import * as yaml from 'js-yaml';
import { parseConfig, type Config, type SupportedChatModel } from './config.js';
import yargs from 'yargs';
import { hideBin } from 'yargs/helpers';

// Model pricing
type ModelPricing = {
  inputCostPerMillion: number;
  outputCostPerMillion: number;
};

const MODEL_PRICING: Record<SupportedChatModel, ModelPricing> = {
  'gpt-4.1': {
    inputCostPerMillion: 2.5,
    outputCostPerMillion: 10,
  },
  'gpt-4o': {
    inputCostPerMillion: 2.5,
    outputCostPerMillion: 10,
  },
  'gpt-4o-mini': {
    inputCostPerMillion: 0.15,
    outputCostPerMillion: 0.6,
  },
  'gpt-5-nano': {
    inputCostPerMillion: 0.05,
    outputCostPerMillion: 0.4,
  },
  'gemini-2.0-flash': {
    inputCostPerMillion: 0.1,
    outputCostPerMillion: 0.4,
  },
  'gemini-2.5-flash': {
    inputCostPerMillion: 0.3,
    outputCostPerMillion: 2.5,
  },
  'gemini-2.5-flash-lite': {
    inputCostPerMillion: 0.1,
    outputCostPerMillion: 0.4,
  },
};

// Generic row data type - can hold any fields
// eslint-disable-next-line @typescript-eslint/no-explicit-any
type RowData = Record<string, any>;

/**
 * Extracts noteId from a row, ensuring it's a valid string or number.
 * Checks multiple possible field names: noteId, id, Id
 * Returns undefined only during validation. After validation, all rows are guaranteed to have an ID.
 */
function getNoteId(row: RowData): string | number | undefined {
  // Check each possible field name in order
  // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
  const noteId = row.noteId ?? row.id ?? row.Id;

  // Ensure the value is actually a string or number (not an object, array, etc.)
  if (typeof noteId === 'string' || typeof noteId === 'number') {
    return noteId;
  }

  // No valid identifier found, or the value is an unexpected type
  return undefined;
}

/**
 * Same as getNoteId but throws if no ID is found.
 * Use this after validation when all rows are guaranteed to have IDs.
 */
function requireNoteId(row: RowData): string | number {
  const noteId = getNoteId(row);
  if (noteId === undefined) {
    throw new Error(
      `Row missing required identifier (noteId, id, or Id). This should not happen after validation. Fields: ${Object.keys(row).join(', ')}`,
    );
  }
  return noteId;
}

// Parse command-line arguments
const argv = yargs(hideBin(process.argv))
  .usage('Usage: $0 <input> <output> <field> <prompt>')
  .command(
    '$0 <input> <output> <field> <prompt>',
    'Process a field in a CSV/YAML file using AI',
  )
  .positional('input', {
    describe: 'Input file path (CSV or YAML)',
    type: 'string',
    demandOption: true,
  })
  .positional('output', {
    describe: 'Output file path (CSV or YAML)',
    type: 'string',
    demandOption: true,
  })
  .positional('field', {
    describe: 'Field name to process with AI',
    type: 'string',
    demandOption: true,
  })
  .positional('prompt', {
    describe: 'Path to prompt template file',
    type: 'string',
    demandOption: true,
  })
  .option('model', {
    alias: 'm',
    describe: 'AI model to use',
    type: 'string',
    default: 'gpt-4o-mini',
    choices: [
      'gpt-4.1',
      'gpt-4o',
      'gpt-4o-mini',
      'gpt-5-nano',
      'gemini-2.0-flash',
      'gemini-2.5-flash',
      'gemini-2.5-flash-lite',
    ],
  })
  .option('batch-size', {
    alias: 'b',
    describe: 'Number of concurrent requests',
    type: 'number',
    default: 5,
  })
  .option('max-tokens', {
    describe: 'Maximum tokens per completion',
    type: 'number',
  })
  .option('temperature', {
    alias: 't',
    describe: 'Sampling temperature (0-2)',
    type: 'number',
    default: 0.3,
  })
  .option('retries', {
    alias: 'r',
    describe: 'Number of retries on failure',
    type: 'number',
    default: 3,
  })
  .option('dry-run', {
    alias: 'd',
    describe: 'Preview without making changes',
    type: 'boolean',
    default: false,
  })
  .option('force', {
    alias: 'f',
    describe: 'Force re-processing of all rows (ignore existing output)',
    type: 'boolean',
    default: false,
  })
  .example(
    '$0 input.csv output.csv english prompt.txt',
    'Process english field with defaults',
  )
  .example(
    '$0 input.yaml out.yaml text prompt.txt -m gpt-4o -t 0.7 -b 10',
    'Custom model, temperature, and batch size',
  )
  .example(
    '$0 data.csv result.csv field prompt.txt --dry-run',
    'Preview without processing',
  )
  .example(
    '$0 data.yaml out.yaml field prompt.txt --force',
    'Re-process all rows (ignore existing output)',
  )
  .epilogue(
    'Environment variables:\n' +
      '  OPENAI_API_KEY or GEMINI_API_KEY - Required: API key for LLM provider',
  )
  .help()
  .parseSync();

/**
 * Fills a template string with data from a row object with robust error handling.
 *
 * This function provides the following guarantees:
 * 1.  **Strictness**: Throws an error if any placeholder in the template does not have a
 *     corresponding key in the data object.
 * 2.  **Case-Insensitivity**: Matches placeholders like `{FieldName}` or `{fieldname}` to
 *     data keys like `fieldName` or `FieldName`.
 * 3.  **Safety**: Detects and throws an error for ambiguous keys in the source data
 *     (e.g., a row with both a 'name' and 'Name' property).
 * 4.  **Efficiency**: Uses a single-pass regex replacement, which is more performant
 *     than iterative methods.
 *
 * @param template The template string with placeholders in `{key}` format.
 * @param row The data object providing values for the placeholders.
 * @returns The processed string with all placeholders replaced.
 * @throws {Error} if the row data contains ambiguous keys (e.g., 'name' and 'Name').
 * @throws {Error} if the template contains placeholders that are not found in the row data.
 */
function fillTemplate(template: string, row: RowData): string {
  // 1. Create a case-insensitive map of the row data to handle variations
  // in key casing (e.g., 'Email' vs. 'email') and detect ambiguities.
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const lowerCaseRow = new Map<string, any>();
  for (const [key, value] of Object.entries(row)) {
    const lowerKey = key.toLowerCase();
    if (lowerCaseRow.has(lowerKey)) {
      // Fail fast on ambiguous keys to prevent unpredictable behavior.
      // Use AbortError to prevent retries on configuration errors
      throw new AbortError(
        `Ambiguous key in row data: "${key}" conflicts with another key when case is ignored.`,
      );
    }
    lowerCaseRow.set(lowerKey, value);
  }

  // 2. Use a regex to find all unique placeholders required by the template.
  const placeholders = [...template.matchAll(/\{([^}]+)\}/g)];
  const requiredKeys = new Set(
    placeholders.map((match) => match[1].toLowerCase()),
  );

  // 3. Validate that all required placeholders exist in the data.
  // This is a critical check to prevent sending incomplete prompts to the LLM.
  const missingKeys: string[] = [];
  for (const key of requiredKeys) {
    if (!lowerCaseRow.has(key)) {
      // Find the original placeholder casing for a more helpful error message.
      const originalPlaceholder = placeholders.find(
        (p) => p[1].toLowerCase() === key,
      )?.[0];
      missingKeys.push(originalPlaceholder || `{${key}}`);
    }
  }

  if (missingKeys.length > 0) {
    // Use AbortError to prevent retries on configuration errors
    throw new AbortError(
      `Missing data for template placeholders: ${missingKeys.join(', ')}. Available fields: ${Object.keys(row).join(', ')}`,
    );
  }

  // 4. Perform the replacement in a single, efficient pass.
  return template.replace(/\{([^}]+)\}/g, (_match, key: string) => {
    const lowerKey = key.toLowerCase();
    // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
    const value = lowerCaseRow.get(lowerKey);
    // Coerce null/undefined to an empty string, preserving the original intent.
    return String(value ?? '');
  });
}

/**
 * Processes a single row using the LLM with retry logic
 */
async function processRow(
  row: RowData,
  fieldToProcess: string,
  promptTemplate: string,
  config: Config,
  client: OpenAI,
  tokenStats: { input: number; output: number },
): Promise<string> {
  const prompt = fillTemplate(promptTemplate, row);

  const response = await client.chat.completions.create({
    model: config.model,
    messages: [
      {
        role: 'user',
        content: prompt,
      },
    ],
    temperature: config.temperature,
    ...(config.maxTokens && { max_tokens: config.maxTokens }),
  });

  const result = response.choices[0]?.message?.content?.trim() || '';

  // Track token usage
  if (response.usage) {
    tokenStats.input += response.usage.prompt_tokens;
    tokenStats.output += response.usage.completion_tokens;
  }

  return result;
}

/**
 * Enhanced row with error tracking
 */
type ProcessedRow = RowData & { _error?: string };

/**
 * Process rows with concurrency control and retry logic
 */
async function processAllRows(
  rows: RowData[],
  fieldToProcess: string,
  promptTemplate: string,
  config: Config,
  client: OpenAI,
  options?: {
    allRows?: RowData[];
    existingRowsMap?: Map<string | number, RowData>;
    outputPath?: string;
  },
): Promise<{
  rows: ProcessedRow[];
  tokenStats: { input: number; output: number };
}> {
  const limit = pLimit(config.batchSize);
  const tokenStats = { input: 0, output: 0 };

  // Create progress bar
  const progressBar = new cliProgress.SingleBar({
    format:
      'Processing |' +
      chalk.cyan('{bar}') +
      '| {percentage}% | {value}/{total} rows | ETA: {eta}s',
    barCompleteChar: '\u2588',
    barIncompleteChar: '\u2591',
    hideCursor: true,
  });

  progressBar.start(rows.length, 0);

  const processedRows: ProcessedRow[] = [];
  const processedMap = new Map<string | number, RowData>();

  // Process in batches and write incrementally
  for (let i = 0; i < rows.length; i += config.batchSize) {
    const batch = rows.slice(i, i + config.batchSize);

    const batchResults = await Promise.all(
      batch.map((row) =>
        limit(async (): Promise<ProcessedRow> => {
          try {
            const processedValue = await pRetry(
              async () => {
                return await processRow(
                  row,
                  fieldToProcess,
                  promptTemplate,
                  config,
                  client,
                  tokenStats,
                );
              },
              {
                retries: config.retries,
                onFailedAttempt: (error) => {
                  // Log retry attempts
                  // Try to get an identifier from the row (id, noteId, or first field)
                  // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
                  const rowId =
                    row.id || row.noteId || Object.values(row)[0] || 'unknown';
                  console.log(
                    chalk.yellow(
                      `\n  Retry ${error.attemptNumber}/${config.retries + 1} for row ${String(rowId)}`,
                    ),
                  );
                },
                // Exponential backoff with jitter
                minTimeout: 1000,
                maxTimeout: 30000,
                factor: 2,
              },
            );

            progressBar.increment();
            return { ...row, [fieldToProcess]: processedValue };
          } catch (error) {
            progressBar.increment();
            const errorMessage =
              error instanceof Error ? error.message : 'Unknown error';
            return { ...row, _error: errorMessage };
          }
        }),
      ),
    );

    processedRows.push(...batchResults);

    // Write incrementally if output path is provided
    if (options?.outputPath && options?.allRows && options?.existingRowsMap) {
      // Update processed map with newly processed rows
      for (const row of batchResults) {
        const noteId = requireNoteId(row);
        processedMap.set(noteId, row);
      }

      // Merge: combine newly processed rows with existing rows (from previous runs)
      // Only include rows that have actually been processed
      const finalRows: RowData[] = [];
      for (const row of options.allRows) {
        const noteId = requireNoteId(row);
        const processedRow =
          processedMap.get(noteId) || options.existingRowsMap.get(noteId);
        if (processedRow) {
          finalRows.push(processedRow);
        }
      }

      // Write to file
      const outputContent = serializeData(finalRows, options.outputPath);
      await writeFile(options.outputPath, outputContent, 'utf-8');
    }
  }

  progressBar.stop();

  return { rows: processedRows, tokenStats };
}

/**
 * Loads existing output file and creates a map of rows by noteId
 * Returns empty map if file doesn't exist
 */
async function loadExistingOutput(
  filePath: string,
): Promise<Map<string | number, RowData>> {
  try {
    const rows = await parseDataFile(filePath);
    const map = new Map<string | number, RowData>();
    for (const row of rows) {
      const noteId = requireNoteId(row);
      map.set(noteId, row);
    }
    return map;
  } catch {
    // File doesn't exist or can't be parsed - return empty map
    return new Map();
  }
}

/**
 * Parses a data file (CSV or YAML) into an array of row objects.
 */
async function parseDataFile(filePath: string): Promise<RowData[]> {
  const fileContent = await readFile(filePath, 'utf-8');
  const extension = path.extname(filePath).toLowerCase();

  if (extension === '.csv') {
    const parseResult = Papa.parse<RowData>(fileContent, {
      header: true,
      skipEmptyLines: true,
    });
    if (parseResult.errors.length > 0) {
      throw new Error(
        `CSV parsing errors: ${JSON.stringify(parseResult.errors)}`,
      );
    }
    return parseResult.data;
  } else if (extension === '.yml' || extension === '.yaml') {
    const data = yaml.load(fileContent);
    // Ensure the YAML content is an array of objects
    if (!Array.isArray(data)) {
      throw new Error('YAML content is not an array');
    }
    // eslint-disable-next-line @typescript-eslint/no-unsafe-return
    return data;
  } else {
    throw new Error(
      `Unsupported file extension: ${extension}. Use .csv, .yaml, or .yml`,
    );
  }
}

/**
 * Serializes an array of row objects to a string (CSV or YAML).
 */
function serializeData(rows: ProcessedRow[], filePath: string): string {
  const extension = path.extname(filePath).toLowerCase();

  if (extension === '.csv') {
    return Papa.unparse(rows, {
      quotes: true,
      newline: '\n',
      header: true,
    });
  } else if (extension === '.yml' || extension === '.yaml') {
    return yaml.dump(rows, { lineWidth: -1 });
  } else {
    throw new Error(
      `Unsupported file extension: ${extension}. Use .csv, .yaml, or .yml`,
    );
  }
}

/**
 * Print summary statistics
 */
function printSummary(
  processedRows: ProcessedRow[],
  tokenStats: { input: number; output: number },
  config: Config,
  elapsedMs: number,
) {
  const failures = processedRows.filter((r) => r._error);
  const successes = processedRows.length - failures.length;

  console.log('\n' + '='.repeat(60));
  console.log(chalk.bold('Summary'));
  console.log('='.repeat(60));
  console.log(chalk.green(`✓ Successful: ${successes}`));
  if (failures.length > 0) {
    console.log(chalk.red(`✗ Failed: ${failures.length}`));
    console.log(chalk.yellow('\nFailed rows:'));
    failures.forEach((row) => {
      // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
      const rowId = row.id || row.noteId || Object.values(row)[0] || 'unknown';
      console.log(chalk.yellow(`  - Row ${String(rowId)}: ${row._error}`));
    });
  }

  console.log(`\n${chalk.bold('Token Usage:')}`);
  console.log(`  Input tokens:  ${tokenStats.input.toLocaleString()}`);
  console.log(`  Output tokens: ${tokenStats.output.toLocaleString()}`);
  console.log(
    `  Total tokens:  ${(tokenStats.input + tokenStats.output).toLocaleString()}`,
  );

  // Get model-specific pricing
  const pricing = MODEL_PRICING[config.model];
  const inputCost =
    (tokenStats.input / 1_000_000) * pricing.inputCostPerMillion;
  const outputCost =
    (tokenStats.output / 1_000_000) * pricing.outputCostPerMillion;
  const totalCost = inputCost + outputCost;

  console.log(`\n${chalk.bold('Cost Breakdown:')}`);
  console.log(`  Model: ${config.model}`);
  console.log(
    `  Input cost:  $${inputCost.toFixed(4)} (${pricing.inputCostPerMillion.toFixed(2)}/M tokens)`,
  );
  console.log(
    `  Output cost: $${outputCost.toFixed(4)} (${pricing.outputCostPerMillion.toFixed(2)}/M tokens)`,
  );
  console.log(chalk.bold(`  Total cost:  $${totalCost.toFixed(4)}`));

  console.log(`\n${chalk.bold('Performance:')}`);
  console.log(`  Total time: ${(elapsedMs / 1000).toFixed(2)}s`);
  console.log(
    `  Avg time per row: ${(elapsedMs / processedRows.length).toFixed(0)}ms`,
  );
}

async function main(): Promise<void> {
  const startTime = Date.now();

  // Parse configuration from CLI args
  const config = parseConfig({
    model: argv.model,
    batchSize: argv.batchSize,
    maxTokens: argv.maxTokens,
    temperature: argv.temperature,
    retries: argv.retries,
    dryRun: argv.dryRun,
  });

  // Read prompt template from file
  const promptTemplate = await readFile(argv.prompt, 'utf-8');

  console.log(chalk.bold('='.repeat(60)));
  console.log(chalk.bold('Batch AI Data Processing'));
  console.log(chalk.bold('='.repeat(60)));
  console.log(`Input file:        ${argv.input}`);
  console.log(`Output file:       ${argv.output}`);
  console.log(`Field to process:  ${argv.field}`);
  console.log(`Model:             ${config.model}`);
  console.log(`Batch size:        ${config.batchSize}`);
  console.log(`Retries:           ${config.retries}`);
  console.log(`Temperature:       ${config.temperature}`);
  if (config.maxTokens) {
    console.log(`Max tokens:        ${config.maxTokens}`);
  }
  console.log(`Dry run:           ${config.dryRun}`);
  console.log(chalk.bold('='.repeat(60)));

  // Read and parse data file
  console.log(`\n${chalk.cyan('Reading')} ${argv.input}...`);
  const rows = await parseDataFile(argv.input);
  const inputFormat = path.extname(argv.input).substring(1).toUpperCase();
  console.log(chalk.green(`✓ Found ${rows.length} rows in ${inputFormat}`));

  if (rows.length === 0) {
    console.log(chalk.yellow('No rows to process. Exiting.'));
    return;
  }

  // Validate field exists
  if (rows.length > 0 && !(argv.field in rows[0])) {
    throw new Error(
      `Field "${argv.field}" not found in input file. Available fields: ${Object.keys(rows[0]).join(', ')}`,
    );
  }

  // Load existing output and filter rows (always enabled unless --force)
  const force = argv.force;

  let existingRowsMap = new Map<string | number, RowData>();
  let rowsToProcess = rows;

  if (!force) {
    console.log(`\n${chalk.cyan('Loading')} existing output...`);
    existingRowsMap = await loadExistingOutput(argv.output);

    if (existingRowsMap.size > 0) {
      console.log(
        chalk.green(`✓ Found ${existingRowsMap.size} already-processed rows`),
      );

      // Filter out rows that are already processed
      rowsToProcess = rows.filter((row) => {
        const noteId = requireNoteId(row);
        return !existingRowsMap.has(noteId);
      });

      const skippedCount = rows.length - rowsToProcess.length;
      if (skippedCount > 0) {
        console.log(
          chalk.yellow(`⏭  Skipping ${skippedCount} already-processed rows`),
        );
      }
    }
  }

  if (rowsToProcess.length === 0) {
    console.log(chalk.green('\n✓ All rows already processed. Nothing to do.'));
    return;
  }

  if (config.dryRun) {
    console.log(chalk.yellow('\n⚠️  DRY RUN MODE - No changes will be saved'));
    console.log(`Would process ${rowsToProcess.length} rows`);
    console.log(`\n${chalk.bold('Prompt template:')}`);
    console.log(promptTemplate);
    console.log(`\n${chalk.bold('Sample row:')}`);
    console.log(JSON.stringify(rowsToProcess[0], null, 2));
    console.log(`\n${chalk.bold('Sample prompt:')}`);
    console.log(fillTemplate(promptTemplate, rowsToProcess[0]));
    return;
  }

  // Initialize OpenAI client
  const client = new OpenAI({
    apiKey: config.apiKey,
    ...(config.apiBaseUrl && { baseURL: config.apiBaseUrl }),
  });

  // Process remaining rows with incremental writing
  console.log(`\n${chalk.cyan('Processing')} ${rowsToProcess.length} rows...`);
  const { rows: processedRows, tokenStats } = await processAllRows(
    rowsToProcess,
    argv.field,
    promptTemplate,
    config,
    client,
    {
      allRows: rows,
      existingRowsMap: existingRowsMap,
      outputPath: argv.output,
    },
  );

  console.log(chalk.green(`\n✓ Processing complete`));

  // Print summary
  const elapsedMs = Date.now() - startTime;
  printSummary(processedRows, tokenStats, config, elapsedMs);
}

// Run main and handle errors
main().catch((error) => {
  if (error instanceof Error) {
    console.error(chalk.red(`\n❌ Error: ${error.message}`));
    if (error instanceof z.ZodError) {
      console.error(chalk.red('Validation details:'));
      console.error(z.prettifyError(error));
    }
  } else {
    console.error(chalk.red('\n❌ An unknown error occurred:'), error);
  }
  process.exit(1);
});

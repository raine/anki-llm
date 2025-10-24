import { readFile, writeFile, rename, mkdtemp, appendFile } from 'fs/promises';
import Papa from 'papaparse';
import { z } from 'zod';
import OpenAI from 'openai';
import pRetry, { AbortError } from 'p-retry';
import pLimit from 'p-limit';
import cliProgress from 'cli-progress';
import chalk from 'chalk';
import * as path from 'path';
import * as yaml from 'js-yaml';
import { tmpdir } from 'os';
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

// Global log file path (set during main())
let logFilePath: string | null = null;

/**
 * Logs a message to both console and log file
 */
async function log(message: string, skipConsole = false): Promise<void> {
  const timestamp = new Date().toISOString();
  const logEntry = `[${timestamp}] ${message}\n`;

  if (!skipConsole) {
    console.log(message);
  }

  if (logFilePath) {
    try {
      await appendFile(logFilePath, logEntry, 'utf-8');
    } catch (error) {
      // Don't let log failures crash the program
      const errorMsg = error instanceof Error ? error.message : String(error);
      console.error(chalk.red(`Failed to write to log file: ${errorMsg}`));
    }
  }
}

/**
 * Atomically writes a file by writing to a temp file first, then renaming.
 * This prevents partial/corrupted files if the process crashes mid-write.
 */
async function atomicWriteFile(filePath: string, data: string): Promise<void> {
  const dir = await mkdtemp(path.join(tmpdir(), 'batch-ai-'));
  const tmpPath = path.join(dir, path.basename(filePath));
  await writeFile(tmpPath, data, 'utf-8');
  await rename(tmpPath, filePath); // atomic on POSIX systems
}

/**
 * Extracts noteId from a row, ensuring it's a valid string or number.
 * Checks multiple possible field names: noteId, id, Id
 * Returns undefined only during validation. After validation, all rows are guaranteed to have an ID.
 * Note: Always normalizes to string to avoid Map key mismatch between '123' and 123.
 */
function getNoteId(row: RowData): string | undefined {
  // Check each possible field name in order
  // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
  const noteId = row.noteId ?? row.id ?? row.Id;

  // Ensure the value is actually a string or number (not an object, array, etc.)
  if (typeof noteId === 'string' || typeof noteId === 'number') {
    // Normalize to string to prevent Map key mismatch ('123' vs 123)
    return String(noteId);
  }

  // No valid identifier found, or the value is an unexpected type
  return undefined;
}

/**
 * Same as getNoteId but throws if no ID is found.
 * Use this after validation when all rows are guaranteed to have IDs.
 */
function requireNoteId(row: RowData): string {
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
  .option('require-result-tag', {
    describe:
      'Require <result></result> XML tags in responses (fail if missing)',
    type: 'boolean',
    default: true,
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
 * Wraps a promise with a timeout. If the promise doesn't resolve within the timeout,
 * it rejects with a timeout error.
 */
function withTimeout<T>(
  promise: Promise<T>,
  timeoutMs: number,
  errorMessage: string,
): Promise<T> {
  return Promise.race([
    promise,
    new Promise<T>((_, reject) =>
      setTimeout(() => reject(new Error(errorMessage)), timeoutMs),
    ),
  ]);
}

/**
 * Calculate total cost from token stats and model pricing
 */
function calculateCost(
  tokenStats: { input: number; output: number },
  model: SupportedChatModel,
): number {
  const pricing = MODEL_PRICING[model];
  const inputCost =
    (tokenStats.input / 1_000_000) * pricing.inputCostPerMillion;
  const outputCost =
    (tokenStats.output / 1_000_000) * pricing.outputCostPerMillion;
  return inputCost + outputCost;
}

/**
 * Extracts content from <result></result> XML tags in the response.
 * If requireResultTag is false, returns the raw response.
 * If requireResultTag is true, throws an error if tags are missing (triggering a retry).
 */
async function parseXmlResult(
  response: string,
  rowId: string,
  requireResultTag: boolean,
): Promise<string> {
  const match = response.match(/<result>([\s\S]*?)<\/result>/);
  if (match && match[1]) {
    const result = match[1].trim();
    await log(
      `Row ${rowId}: Successfully parsed result from XML tags (${result.length} chars)`,
      true,
    );
    return result;
  }

  // No XML tags found
  if (requireResultTag) {
    // Strict mode: throw error to trigger retry
    const errorMsg = `Row ${rowId}: Response missing required <result></result> tags. Full response: ${response}`;
    await log(errorMsg, true);
    console.log(
      chalk.yellow(
        `\n  ⚠️  Response missing <result></result> tags. Full response:\n${chalk.gray(response)}`,
      ),
    );
    throw new Error(
      `Response missing required <result></result> tags. Response preview: ${response.substring(0, 100)}...`,
    );
  } else {
    // Permissive mode: use raw response
    return response.trim();
  }
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
  const rowId = requireNoteId(row);

  await log(`Row ${rowId}: Starting processing`, true);

  const prompt = fillTemplate(promptTemplate, row);
  await log(`Row ${rowId}: Generated prompt (${prompt.length} chars)`, true);

  await log(`Row ${rowId}: Sending request to ${config.model}`, true);

  // Track request timing
  const requestStartTime = Date.now();

  // Add 60 second timeout to API request to prevent infinite hangs
  const response = await withTimeout(
    client.chat.completions.create({
      model: config.model,
      messages: [
        {
          role: 'user',
          content: prompt,
        },
      ],
      temperature: config.temperature,
      ...(config.maxTokens && { max_tokens: config.maxTokens }),
    }),
    60000, // 60 second timeout
    `Request timeout after 60 seconds for row ${rowId}`,
  );

  const requestDurationMs = Date.now() - requestStartTime;
  const rawResult = response.choices[0]?.message?.content?.trim() || '';
  await log(
    `Row ${rowId}: Received response (${rawResult.length} chars) in ${requestDurationMs}ms (${(requestDurationMs / 1000).toFixed(2)}s)`,
    true,
  );

  // Track token usage
  if (response.usage) {
    tokenStats.input += response.usage.prompt_tokens;
    tokenStats.output += response.usage.completion_tokens;
    await log(
      `Row ${rowId}: Token usage - Input: ${response.usage.prompt_tokens}, Output: ${response.usage.completion_tokens}`,
      true,
    );
  }

  // Parse XML to extract result from <result></result> tags (or use raw response)
  const result = await parseXmlResult(
    rawResult,
    rowId,
    config.requireResultTag,
  );

  await log(`Row ${rowId}: Processing complete`, true);
  return result;
}

/**
 * Enhanced row with error tracking
 */
type ProcessedRow = RowData & { _error?: string };

/**
 * Process rows with concurrency control and retry logic without batch-blocking.
 */
async function processAllRows(
  rows: RowData[],
  fieldToProcess: string,
  promptTemplate: string,
  config: Config,
  client: OpenAI,
  options?: {
    allRows?: RowData[];
    existingRowsMap?: Map<string, RowData>;
    outputPath?: string;
  },
): Promise<{
  rows: ProcessedRow[];
  tokenStats: { input: number; output: number };
}> {
  const limit = pLimit(config.batchSize);
  const tokenStats = { input: 0, output: 0 };

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

  // This array will store results in the original order.
  const orderedResults: ProcessedRow[] = new Array<ProcessedRow>(rows.length);

  // --- Logic for buffered incremental writing ---
  const processedMap = new Map<string, RowData>();
  const completedBuffer: ProcessedRow[] = [];
  let completedCount = 0; // Track how many rows have completed

  const performIncrementalWrite = async () => {
    if (!options?.outputPath || !options?.allRows || !options?.existingRowsMap)
      return;

    // Update the central map with the buffered results
    for (const row of completedBuffer) {
      processedMap.set(requireNoteId(row), row);
    }
    completedBuffer.length = 0; // Clear buffer

    // Merge new results with existing ones to create the full output file.
    // Note: This preserves the existing behavior of only writing rows that
    // have a processed result (from this run or a previous one).
    const finalRows: RowData[] = [];
    for (const row of options.allRows) {
      const noteId = requireNoteId(row);
      const processedRow =
        processedMap.get(noteId) || options.existingRowsMap.get(noteId);
      if (processedRow) {
        finalRows.push(processedRow);
      }
    }

    // Write to file atomically
    const outputContent = serializeData(finalRows, options.outputPath);
    await atomicWriteFile(options.outputPath, outputContent);
  };
  // --- End of incremental writing logic ---

  const allPromises = rows.map((row, index) =>
    limit(async () => {
      let result: ProcessedRow;
      try {
        const processedValue = await pRetry(
          () =>
            processRow(
              row,
              fieldToProcess,
              promptTemplate,
              config,
              client,
              tokenStats,
            ),
          {
            retries: config.retries,
            onFailedAttempt: async (error) => {
              const rowId = requireNoteId(row);
              const errorMsg =
                error instanceof Error ? error.message : 'Unknown error';
              const retryMsg = `Retry ${error.attemptNumber}/${config.retries + 1} for row ${rowId}: ${errorMsg}`;
              console.log(chalk.yellow(`\n  ${retryMsg}`));
              await log(retryMsg, true);
            },
            minTimeout: 1000,
            maxTimeout: 30000,
            factor: 2,
          },
        );
        result = { ...row, [fieldToProcess]: processedValue };
      } catch (error) {
        if (error instanceof AbortError) {
          throw error; // Critical error, stop everything
        }
        const errorMessage =
          error instanceof Error ? error.message : 'Unknown error';
        const rowId = requireNoteId(row);
        await log(
          `Row ${rowId}: FAILED after all retries - ${errorMessage}`,
          true,
        );
        result = { ...row, _error: errorMessage };
      }

      // Store result in order and handle incremental writing
      orderedResults[index] = result;
      completedCount++;

      // Log cost every 10 completed rows
      if (completedCount % 10 === 0) {
        const currentCost = calculateCost(tokenStats, config.model);
        await log(
          `Progress: ${completedCount} rows completed | Tokens: ${tokenStats.input + tokenStats.output} (in: ${tokenStats.input}, out: ${tokenStats.output}) | Cost so far: $${currentCost.toFixed(4)}`,
          true,
        );
      }

      if (options?.outputPath) {
        completedBuffer.push(result);
        if (completedBuffer.length >= config.batchSize) {
          await performIncrementalWrite();
        }
      }

      progressBar.increment();
    }),
  );

  // Wait for all queued promises to complete
  await Promise.all(allPromises);

  // Write any remaining items in the buffer
  if (completedBuffer.length > 0) {
    await performIncrementalWrite();
  }

  progressBar.stop();

  return { rows: orderedResults, tokenStats };
}

/**
 * Loads existing output file and creates a map of rows by noteId
 * Returns empty map if file doesn't exist
 */
async function loadExistingOutput(
  filePath: string,
): Promise<Map<string, RowData>> {
  try {
    const rows = await parseDataFile(filePath);
    const map = new Map<string, RowData>();
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

  // Initialize log file path
  const outputDir = path.dirname(argv.output);
  const outputName = path.basename(argv.output, path.extname(argv.output));
  logFilePath = path.join(outputDir, `${outputName}.log`);

  // Clear previous log file
  await writeFile(logFilePath, '', 'utf-8');

  await log('='.repeat(60));
  await log('Batch AI Data Processing - Session Started');
  await log('='.repeat(60));

  // Parse configuration from CLI args
  const config = parseConfig({
    model: argv.model,
    batchSize: argv.batchSize,
    maxTokens: argv.maxTokens,
    temperature: argv.temperature,
    retries: argv.retries,
    dryRun: argv.dryRun,
    requireResultTag: argv.requireResultTag,
  });

  await log(`Log file: ${logFilePath}`);
  await log(`Input file: ${argv.input}`);
  await log(`Output file: ${argv.output}`);
  await log(`Field to process: ${argv.field}`);
  await log(`Model: ${config.model}`);
  await log(`Batch size: ${config.batchSize}`);
  await log(`Retries: ${config.retries}`);
  await log(`Temperature: ${config.temperature}`);
  if (config.maxTokens) {
    await log(`Max tokens: ${config.maxTokens}`);
  }
  await log(`Dry run: ${config.dryRun}`);
  await log(`Require result tag: ${config.requireResultTag}`);

  // Read prompt template from file
  const promptTemplate = await readFile(argv.prompt, 'utf-8');
  await log(`Prompt template loaded (${promptTemplate.length} chars)`);

  console.log(chalk.bold('='.repeat(60)));
  console.log(chalk.bold('Batch AI Data Processing'));
  console.log(chalk.bold('='.repeat(60)));
  console.log(`Input file:        ${argv.input}`);
  console.log(`Output file:       ${argv.output}`);
  console.log(`Log file:          ${logFilePath}`);
  console.log(`Field to process:  ${argv.field}`);
  console.log(`Model:             ${config.model}`);
  console.log(`Batch size:        ${config.batchSize}`);
  console.log(`Retries:           ${config.retries}`);
  console.log(`Temperature:       ${config.temperature}`);
  if (config.maxTokens) {
    console.log(`Max tokens:        ${config.maxTokens}`);
  }
  console.log(`Dry run:           ${config.dryRun}`);
  console.log(`Require result tag: ${config.requireResultTag}`);
  console.log(chalk.bold('='.repeat(60)));

  // Read and parse data file
  console.log(`\n${chalk.cyan('Reading')} ${argv.input}...`);
  await log(`Reading input file: ${argv.input}`);
  const rows = await parseDataFile(argv.input);
  const inputFormat = path.extname(argv.input).substring(1).toUpperCase();
  console.log(chalk.green(`✓ Found ${rows.length} rows in ${inputFormat}`));
  await log(`Parsed ${rows.length} rows from ${inputFormat} file`);

  if (rows.length === 0) {
    console.log(chalk.yellow('No rows to process. Exiting.'));
    await log('No rows found. Exiting.');
    return;
  }

  // Validate field exists
  await log('Validating field exists in input data');
  if (rows.length > 0 && !(argv.field in rows[0])) {
    const error = `Field "${argv.field}" not found in input file. Available fields: ${Object.keys(rows[0]).join(', ')}`;
    await log(`ERROR: ${error}`);
    throw new Error(error);
  }
  await log(`Field "${argv.field}" validated successfully`);

  // Validate no duplicate noteIds in input
  await log('Validating no duplicate noteIds in input');
  const seenIds = new Set<string>();
  for (let i = 0; i < rows.length; i++) {
    const noteId = requireNoteId(rows[i]);
    if (seenIds.has(noteId)) {
      const error = `Duplicate noteId "${noteId}" detected in input (first seen at row ${seenIds.size + 1}, duplicate at row ${i + 1})`;
      await log(`ERROR: ${error}`);
      throw new AbortError(error);
    }
    seenIds.add(noteId);
  }
  await log('No duplicate noteIds found');

  // Load existing output and filter rows (always enabled unless --force)
  const force = argv.force;

  let existingRowsMap = new Map<string, RowData>();
  let rowsToProcess = rows;

  if (!force) {
    console.log(`\n${chalk.cyan('Loading')} existing output...`);
    await log('Loading existing output to skip already-processed rows');
    existingRowsMap = await loadExistingOutput(argv.output);

    if (existingRowsMap.size > 0) {
      console.log(
        chalk.green(`✓ Found ${existingRowsMap.size} already-processed rows`),
      );
      await log(`Found ${existingRowsMap.size} already-processed rows`);

      // Filter out rows that are already processed (skip rows with errors - retry them)
      rowsToProcess = rows.filter((row) => {
        const noteId = requireNoteId(row);
        const existing = existingRowsMap.get(noteId);
        // Process if: no existing row, or existing row has an error
        return !existing || existing._error;
      });

      const skippedCount = rows.length - rowsToProcess.length;
      if (skippedCount > 0) {
        console.log(
          chalk.yellow(`⏭  Skipping ${skippedCount} already-processed rows`),
        );
        await log(`Skipping ${skippedCount} already-processed rows`);
      }
    } else {
      await log('No existing output found, will process all rows');
    }
  } else {
    await log('Force mode enabled, will re-process all rows');
  }

  if (rowsToProcess.length === 0) {
    console.log(chalk.green('\n✓ All rows already processed. Nothing to do.'));
    await log('All rows already processed. Exiting.');
    return;
  }

  await log(`Total rows to process: ${rowsToProcess.length}`);

  if (config.dryRun) {
    console.log(chalk.yellow('\n⚠️  DRY RUN MODE - No changes will be saved'));
    console.log(`Would process ${rowsToProcess.length} rows`);
    console.log(`\n${chalk.bold('Prompt template:')}`);
    console.log(promptTemplate);
    console.log(`\n${chalk.bold('Sample row:')}`);
    console.log(JSON.stringify(rowsToProcess[0], null, 2));
    console.log(`\n${chalk.bold('Sample prompt:')}`);
    console.log(fillTemplate(promptTemplate, rowsToProcess[0]));
    await log('Dry run complete. Exiting.');
    return;
  }

  // Initialize OpenAI client
  await log(`Initializing OpenAI client for model: ${config.model}`);
  const client = new OpenAI({
    apiKey: config.apiKey,
    ...(config.apiBaseUrl && { baseURL: config.apiBaseUrl }),
  });

  // Process remaining rows with incremental writing
  console.log(`\n${chalk.cyan('Processing')} ${rowsToProcess.length} rows...`);
  await log(`Starting batch processing of ${rowsToProcess.length} rows`);
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
  await log('Batch processing complete');

  // Print summary
  const elapsedMs = Date.now() - startTime;
  printSummary(processedRows, tokenStats, config, elapsedMs);

  // Log summary to file
  const failures = processedRows.filter((r) => r._error);
  const successes = processedRows.length - failures.length;
  await log(`Summary: ${successes} successful, ${failures.length} failed`);
  await log(
    `Total tokens: ${tokenStats.input + tokenStats.output} (input: ${tokenStats.input}, output: ${tokenStats.output})`,
  );
  await log(`Total time: ${(elapsedMs / 1000).toFixed(2)}s`);
  await log('Session completed successfully');
}

// Run main and handle errors
main().catch(async (error) => {
  if (error instanceof Error) {
    console.error(chalk.red(`\n❌ Error: ${error.message}`));
    if (logFilePath) {
      await log(`FATAL ERROR: ${error.message}`);
      await log(`Stack trace: ${error.stack}`);
    }
    if (error instanceof z.ZodError) {
      console.error(chalk.red('Validation details:'));
      console.error(z.prettifyError(error));
    }
  } else {
    console.error(chalk.red('\n❌ An unknown error occurred:'), error);
    if (logFilePath) {
      await log(`FATAL ERROR: Unknown error - ${String(error)}`);
    }
  }
  process.exit(1);
});

import { readFile, writeFile } from 'fs/promises';
import Papa from 'papaparse';
import { z } from 'zod';
import OpenAI from 'openai';
import pRetry from 'p-retry';
import pLimit from 'p-limit';
import cliProgress from 'cli-progress';
import chalk from 'chalk';

// Supported models and their pricing
type ModelPricing = {
  inputCostPerMillion: number;
  outputCostPerMillion: number;
};

type SupportedChatModel =
  | 'gpt-4.1'
  | 'gpt-4o'
  | 'gpt-4o-mini'
  | 'gpt-5-nano'
  | 'gemini-2.0-flash'
  | 'gemini-2.5-flash'
  | 'gemini-2.5-flash-lite-preview-06-17';

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
    inputCostPerMillion: 0.15,
    outputCostPerMillion: 0.6,
  },
  'gemini-2.5-flash-lite-preview-06-17': {
    inputCostPerMillion: 0.1,
    outputCostPerMillion: 0.4,
  },
};

// Zod schema for CSV rows
const CsvRow = z.object({
  id: z.string(),
  english: z.string(),
  japanese: z.string(),
  ka: z.string(),
  ROM: z.string(),
  explanation: z.string(),
});
type CsvRow = z.infer<typeof CsvRow>;

// Define valid field names for type safety
const CsvFieldName = z.enum([
  'id',
  'english',
  'japanese',
  'ka',
  'ROM',
  'explanation',
]);
type CsvFieldName = z.infer<typeof CsvFieldName>;

// CLI arguments schema
const CliArgs = z.tuple([
  z.string().min(1, 'Input file is required'),
  z.string().min(1, 'Output file is required'),
  CsvFieldName,
  z.string().min(1, 'Prompt template is required'),
]);

// Configuration schema
const SupportedModelSchema = z.enum([
  'gpt-4.1',
  'gpt-4o',
  'gpt-4o-mini',
  'gpt-5-nano',
  'gemini-2.0-flash',
  'gemini-2.5-flash',
  'gemini-2.5-flash-lite-preview-06-17',
]);

const Config = z.object({
  apiKey: z.string().min(1, 'API key is required'),
  apiBaseUrl: z.string().optional(),
  model: SupportedModelSchema.default('gpt-4o-mini'),
  batchSize: z.number().int().positive().default(5),
  dryRun: z.boolean().default(false),
  maxTokens: z.number().int().positive().optional(),
  temperature: z.number().min(0).max(2).default(0.3),
  systemPrompt: z.string().optional(),
  retries: z.number().int().min(0).default(3),
});
type Config = z.infer<typeof Config>;

// Parse and validate CLI arguments
function parseCliArgs(): [string, string, CsvFieldName, string] {
  const args = process.argv.slice(2);

  if (args.length < 4) {
    console.error(
      chalk.red(
        'Usage: tsx src/batch-ai-process-csv.ts <input.csv> <output.csv> <field_to_process> <prompt_file>',
      ),
    );
    console.error('\nExample:');
    console.error(
      '  tsx src/batch-ai-process-csv.ts input.csv output.csv english src/prompts/translation-japanese-to-english.txt',
    );
    console.error('\nValid fields:', CsvFieldName.options.join(', '));
    console.error('\nEnvironment variables:');
    console.error(
      '  OPENAI_API_KEY or GEMINI_API_KEY - Required: API key for the LLM provider',
    );
    console.error(
      '  OPENAI_API_BASE or API_BASE_URL - Optional: Base URL for API',
    );
    console.error('  MODEL - Optional: Model to use (default: gpt-4o-mini)');
    console.error(`    Supported: ${SupportedModelSchema.options.join(', ')}`);
    console.error(
      '  BATCH_SIZE - Optional: Number of concurrent requests (default: 5)',
    );
    console.error('  MAX_TOKENS - Optional: Maximum tokens per completion');
    console.error(
      '  TEMPERATURE - Optional: Temperature for sampling (default: 0.3)',
    );
    console.error(
      '  SYSTEM_PROMPT - Optional: System message to include before user prompt',
    );
    console.error(
      '  RETRIES - Optional: Number of retries on failure (default: 3)',
    );
    console.error(
      '  DRY_RUN - Optional: Set to "true" to preview without making changes',
    );
    process.exit(1);
  }

  try {
    return CliArgs.parse(args);
  } catch (error) {
    if (error instanceof z.ZodError) {
      console.error(chalk.red('Invalid arguments:'));
      for (const issue of error.issues) {
        console.error(
          chalk.red(`  - ${issue.path.join('.')}: ${issue.message}`),
        );
      }
    }
    process.exit(1);
  }
}

/**
 * Determines the provider and base URL based on the model name
 */
function getProviderConfig(model: SupportedChatModel): {
  baseURL?: string;
  recommendedApiKeyEnv: string;
} {
  if (model.startsWith('gpt-')) {
    return {
      recommendedApiKeyEnv: 'OPENAI_API_KEY',
    };
  } else if (model.startsWith('gemini-')) {
    return {
      baseURL: 'https://generativelanguage.googleapis.com/v1beta/openai/',
      recommendedApiKeyEnv: 'GEMINI_API_KEY',
    };
  }
  return {
    recommendedApiKeyEnv: 'OPENAI_API_KEY',
  };
}

// Parse and validate configuration from environment
function parseConfig(): Config {
  const dryRun = process.env.DRY_RUN === 'true';

  // Get model first to determine which API key to look for
  const modelStr = process.env.MODEL || 'gpt-4o-mini';
  let model: SupportedChatModel;
  try {
    model = SupportedModelSchema.parse(modelStr);
  } catch {
    console.error(chalk.red(`Invalid MODEL: ${modelStr}`));
    console.error(
      chalk.yellow(
        `Supported models: ${SupportedModelSchema.options.join(', ')}`,
      ),
    );
    process.exit(1);
  }

  const providerConfig = getProviderConfig(model);

  // Check for API key
  const apiKey = process.env.OPENAI_API_KEY || process.env.GEMINI_API_KEY;

  // Skip API key check in dry run mode
  if (!dryRun && !apiKey) {
    console.error(
      chalk.red(
        'Error: OPENAI_API_KEY or GEMINI_API_KEY environment variable is required',
      ),
    );
    console.error(
      chalk.yellow(
        `Tip: For model '${model}', set ${providerConfig.recommendedApiKeyEnv}`,
      ),
    );
    console.error(
      chalk.yellow('Or set DRY_RUN=true to preview without an API key'),
    );
    process.exit(1);
  }

  // Determine base URL: explicit override > provider default > undefined
  const apiBaseUrl =
    process.env.OPENAI_API_BASE ||
    process.env.API_BASE_URL ||
    providerConfig.baseURL;

  try {
    return Config.parse({
      apiKey: apiKey || 'dummy-key-for-dry-run',
      apiBaseUrl,
      model,
      batchSize: process.env.BATCH_SIZE
        ? parseInt(process.env.BATCH_SIZE, 10)
        : undefined,
      dryRun,
      maxTokens: process.env.MAX_TOKENS
        ? parseInt(process.env.MAX_TOKENS, 10)
        : undefined,
      temperature: process.env.TEMPERATURE
        ? parseFloat(process.env.TEMPERATURE)
        : undefined,
      systemPrompt: process.env.SYSTEM_PROMPT,
      retries: process.env.RETRIES
        ? parseInt(process.env.RETRIES, 10)
        : undefined,
    });
  } catch (error) {
    if (error instanceof z.ZodError) {
      console.error(chalk.red('Invalid configuration:'));
      for (const issue of error.issues) {
        console.error(
          chalk.red(`  - ${issue.path.join('.')}: ${issue.message}`),
        );
      }
    }
    process.exit(1);
  }
}

/**
 * Safely replaces placeholders in template with values from row
 * Uses simple string splitting to avoid regex replacement pattern issues
 */
function fillTemplate(template: string, row: CsvRow): string {
  let result = template;
  for (const [key, value] of Object.entries(row)) {
    // Use split/join to avoid regex replacement pattern issues with $ and \
    result = result.split(`{${key}}`).join(value);
  }
  return result;
}

/**
 * Processes a single row using the LLM with retry logic
 */
async function processRow(
  row: CsvRow,
  fieldToProcess: CsvFieldName,
  promptTemplate: string,
  config: Config,
  client: OpenAI,
  tokenStats: { input: number; output: number },
): Promise<string> {
  const prompt = fillTemplate(promptTemplate, row);

  // Build messages array - only include system prompt if provided
  const messages: OpenAI.Chat.ChatCompletionMessageParam[] = [];
  if (config.systemPrompt) {
    messages.push({
      role: 'system',
      content: config.systemPrompt,
    });
  }
  messages.push({
    role: 'user',
    content: prompt,
  });

  const response = await client.chat.completions.create({
    model: config.model,
    messages,
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
 * Enhanced CSV row with error tracking
 */
type ProcessedRow = CsvRow & { _error?: string };

/**
 * Process rows with concurrency control and retry logic
 */
async function processAllRows(
  rows: CsvRow[],
  fieldToProcess: CsvFieldName,
  promptTemplate: string,
  config: Config,
  client: OpenAI,
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

  const processedRows = await Promise.all(
    rows.map((row) =>
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
                console.log(
                  chalk.yellow(
                    `\n  Retry ${error.attemptNumber}/${config.retries + 1} for row ${row.id}`,
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

  progressBar.stop();

  return { rows: processedRows, tokenStats };
}

/**
 * Converts rows to CSV using Papa.unparse for proper escaping
 */
function rowsToCsv(rows: ProcessedRow[]): string {
  return Papa.unparse(rows, {
    quotes: true,
    newline: '\n',
    header: true,
  });
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
      console.log(chalk.yellow(`  - Row ${row.id}: ${row._error}`));
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

  // Parse CLI arguments and configuration
  const [INPUT_FILE, OUTPUT_FILE, FIELD_TO_PROCESS, PROMPT_FILE] =
    parseCliArgs();
  const config = parseConfig();

  // Read prompt template from file
  const PROMPT_TEMPLATE = await readFile(PROMPT_FILE, 'utf-8');

  console.log(chalk.bold('='.repeat(60)));
  console.log(chalk.bold('Batch AI CSV Processing'));
  console.log(chalk.bold('='.repeat(60)));
  console.log(`Input file:        ${INPUT_FILE}`);
  console.log(`Output file:       ${OUTPUT_FILE}`);
  console.log(`Field to process:  ${FIELD_TO_PROCESS}`);
  console.log(`Model:             ${config.model}`);
  console.log(`Batch size:        ${config.batchSize}`);
  console.log(`Retries:           ${config.retries}`);
  console.log(`Temperature:       ${config.temperature}`);
  if (config.maxTokens) {
    console.log(`Max tokens:        ${config.maxTokens}`);
  }
  console.log(`Dry run:           ${config.dryRun}`);
  console.log(chalk.bold('='.repeat(60)));

  // Read and parse CSV
  console.log(`\n${chalk.cyan('Reading')} ${INPUT_FILE}...`);
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

  const rows = parseResult.data.map((row) => CsvRow.parse(row));
  console.log(chalk.green(`✓ Found ${rows.length} rows in CSV`));

  if (rows.length === 0) {
    console.log(chalk.yellow('No rows to process. Exiting.'));
    return;
  }

  // Validate field exists
  if (rows.length > 0 && !(FIELD_TO_PROCESS in rows[0])) {
    throw new Error(
      `Field "${FIELD_TO_PROCESS}" not found in CSV. Available fields: ${Object.keys(rows[0]).join(', ')}`,
    );
  }

  if (config.dryRun) {
    console.log(chalk.yellow('\n⚠️  DRY RUN MODE - No changes will be saved'));
    console.log(`Would process ${rows.length} rows`);
    console.log(`\n${chalk.bold('Prompt template:')}`);
    console.log(PROMPT_TEMPLATE);
    console.log(`\n${chalk.bold('Sample row:')}`);
    console.log(JSON.stringify(rows[0], null, 2));
    console.log(`\n${chalk.bold('Sample prompt:')}`);
    console.log(fillTemplate(PROMPT_TEMPLATE, rows[0]));
    return;
  }

  // Initialize OpenAI client
  const client = new OpenAI({
    apiKey: config.apiKey,
    ...(config.apiBaseUrl && { baseURL: config.apiBaseUrl }),
  });

  // Process all rows
  console.log(`\n${chalk.cyan('Processing')} ${rows.length} rows...`);
  const { rows: processedRows, tokenStats } = await processAllRows(
    rows,
    FIELD_TO_PROCESS,
    PROMPT_TEMPLATE,
    config,
    client,
  );

  // Write results to CSV
  console.log(`\n${chalk.cyan('Writing')} results to ${OUTPUT_FILE}...`);
  const csvContent = rowsToCsv(processedRows);
  await writeFile(OUTPUT_FILE, csvContent, 'utf-8');
  console.log(
    chalk.green(
      `✓ Successfully wrote ${processedRows.length} rows to ${OUTPUT_FILE}`,
    ),
  );

  // Print summary
  const elapsedMs = Date.now() - startTime;
  printSummary(processedRows, tokenStats, config, elapsedMs);
}

// Run main and handle errors
main().catch((error) => {
  if (error instanceof Error) {
    console.error(chalk.red(`\n✗ Error: ${error.message}`));
    if (error instanceof z.ZodError) {
      console.error(chalk.red('Validation details:'));
      console.error(z.flattenError(error));
    }
  } else {
    console.error(chalk.red('\n✗ An unknown error occurred:'), error);
  }
  process.exit(1);
});

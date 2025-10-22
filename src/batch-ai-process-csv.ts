import { readFile, writeFile } from 'fs/promises';
import Papa from 'papaparse';
import { z } from 'zod';
import OpenAI from 'openai';

// Parse command-line arguments
const args = process.argv.slice(2);
const INPUT_FILE = args[0];
const OUTPUT_FILE = args[1];
const FIELD_TO_PROCESS = args[2]; // e.g., "english"
const PROMPT_TEMPLATE = args[3]; // The prompt to use for processing

// Environment variables for API configuration
const API_KEY = process.env.OPENAI_API_KEY || process.env.GEMINI_API_KEY;
const API_BASE_URL = process.env.API_BASE_URL; // Optional: for Gemini or other providers
const MODEL = process.env.MODEL || 'gpt-4o-mini'; // Default model

// Validate command-line arguments
if (!INPUT_FILE || !OUTPUT_FILE || !FIELD_TO_PROCESS || !PROMPT_TEMPLATE) {
  console.error(
    'Usage: tsx src/batch-ai-process-csv.ts <input.csv> <output.csv> <field_to_process> <prompt_template>',
  );
  console.error('\nExample:');
  console.error(
    '  tsx src/batch-ai-process-csv.ts input.csv output.csv english "Translate the following Japanese to English: {japanese}"',
  );
  console.error('\nEnvironment variables:');
  console.error(
    '  OPENAI_API_KEY or GEMINI_API_KEY - Required: API key for the LLM provider',
  );
  console.error(
    '  API_BASE_URL - Optional: Base URL for API (e.g., for Gemini)',
  );
  console.error('  MODEL - Optional: Model to use (default: gpt-4o-mini)');
  console.error(
    '  BATCH_SIZE - Optional: Number of concurrent requests (default: 5)',
  );
  console.error(
    '  DRY_RUN - Optional: Set to "true" to preview without making changes',
  );
  process.exit(1);
}

if (!API_KEY) {
  console.error(
    'Error: OPENAI_API_KEY or GEMINI_API_KEY environment variable is required',
  );
  process.exit(1);
}

const BATCH_SIZE = parseInt(process.env.BATCH_SIZE || '5', 10);
const DRY_RUN = process.env.DRY_RUN === 'true';

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

// Initialize OpenAI client (works with OpenAI-compatible APIs)
const client = new OpenAI({
  apiKey: API_KEY,
  ...(API_BASE_URL && { baseURL: API_BASE_URL }),
});

/**
 * Processes a single row using the LLM
 */
async function processRow(
  row: CsvRow,
  fieldToProcess: keyof CsvRow,
  promptTemplate: string,
): Promise<string> {
  // Replace placeholders in the prompt template with actual values from the row
  let prompt = promptTemplate;
  for (const [key, value] of Object.entries(row)) {
    prompt = prompt.replace(new RegExp(`\\{${key}\\}`, 'g'), value);
  }

  try {
    const response = await client.chat.completions.create({
      model: MODEL,
      messages: [
        {
          role: 'system',
          content: `You are an assistant for translating Japanese to English.

- Translate the given sentence into English as accurately as possible, preserving the original nuance and meaning.
- Do not say at the start that below is translation of the sentence, just proceed to translation
- Try to incorporate nuances of Japanese language into the translation, so that it's easier to figure out the original Japanese sentence based on the English translation`,
        },
        {
          role: 'user',
          content: prompt,
        },
      ],
      temperature: 0.3, // Lower temperature for more consistent translations
    });

    const result = response.choices[0]?.message?.content?.trim() || '';

    // Log token usage
    if (response.usage) {
      console.log(
        `  [Row ${row.id}] Tokens: ${response.usage.prompt_tokens} in, ${response.usage.completion_tokens} out`,
      );
    }

    return result;
  } catch (error) {
    console.error(`Error processing row ${row.id}:`, error);
    throw error;
  }
}

/**
 * Process rows in batches to avoid overwhelming the API
 */
async function processBatch(
  rows: CsvRow[],
  fieldToProcess: keyof CsvRow,
  promptTemplate: string,
): Promise<CsvRow[]> {
  const results: CsvRow[] = [];

  for (let i = 0; i < rows.length; i += BATCH_SIZE) {
    const batch = rows.slice(i, i + BATCH_SIZE);
    console.log(
      `\nProcessing batch ${Math.floor(i / BATCH_SIZE) + 1}/${Math.ceil(rows.length / BATCH_SIZE)} (rows ${i + 1}-${i + batch.length})...`,
    );

    const batchResults = await Promise.all(
      batch.map(async (row) => {
        try {
          const processedValue = await processRow(
            row,
            fieldToProcess,
            promptTemplate,
          );
          const updatedRow = { ...row, [fieldToProcess]: processedValue };

          console.log(
            `  ✓ Row ${row.id}: "${row[fieldToProcess]}" → "${processedValue}"`,
          );

          return updatedRow;
        } catch {
          console.error(`  ✗ Row ${row.id}: Failed to process`);
          return row; // Return original row on error
        }
      }),
    );

    results.push(...batchResults);

    // Small delay between batches to be respectful to API rate limits
    if (i + BATCH_SIZE < rows.length) {
      await new Promise((resolve) => setTimeout(resolve, 1000));
    }
  }

  return results;
}

/**
 * Converts an array of rows to CSV format
 */
function rowsToCsv(rows: CsvRow[]): string {
  const fieldNames: (keyof CsvRow)[] = [
    'id',
    'english',
    'japanese',
    'ka',
    'ROM',
    'explanation',
  ];

  // CSV header
  const header = fieldNames.join(',');

  // CSV rows
  const csvRows = rows.map((row) => {
    return fieldNames
      .map((field) => {
        const value = row[field];
        // Escape quotes and wrap in quotes if the value contains comma, quote, or newline
        if (
          value.includes(',') ||
          value.includes('"') ||
          value.includes('\n')
        ) {
          return `"${value.replace(/"/g, '""')}"`;
        }
        return value;
      })
      .join(',');
  });

  return [header, ...csvRows].join('\n');
}

async function main(): Promise<void> {
  console.log('='.repeat(60));
  console.log('Batch AI CSV Processing');
  console.log('='.repeat(60));
  console.log(`Input file: ${INPUT_FILE}`);
  console.log(`Output file: ${OUTPUT_FILE}`);
  console.log(`Field to process: ${FIELD_TO_PROCESS}`);
  console.log(`Model: ${MODEL}`);
  console.log(`Batch size: ${BATCH_SIZE}`);
  console.log(`Dry run: ${DRY_RUN}`);
  console.log('='.repeat(60));

  try {
    // Read and parse CSV
    console.log(`\nReading ${INPUT_FILE}...`);
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
    console.log(`✓ Found ${rows.length} rows in CSV`);

    if (rows.length === 0) {
      console.log('No rows to process. Exiting.');
      return;
    }

    // Validate field exists
    if (!(FIELD_TO_PROCESS in rows[0])) {
      throw new Error(
        `Field "${FIELD_TO_PROCESS}" not found in CSV. Available fields: ${Object.keys(rows[0]).join(', ')}`,
      );
    }

    if (DRY_RUN) {
      console.log('\n⚠️  DRY RUN MODE - No changes will be saved');
      console.log(
        `Would process ${rows.length} rows with prompt: "${PROMPT_TEMPLATE}"`,
      );
      console.log('\nSample row:');
      console.log(JSON.stringify(rows[0], null, 2));
      return;
    }

    // Process all rows
    const processedRows = await processBatch(
      rows,
      FIELD_TO_PROCESS as keyof CsvRow,
      PROMPT_TEMPLATE,
    );

    // Write results to CSV
    console.log(`\nWriting results to ${OUTPUT_FILE}...`);
    const csvContent = rowsToCsv(processedRows);
    await writeFile(OUTPUT_FILE, csvContent, 'utf-8');

    console.log(`✓ Successfully processed ${processedRows.length} rows`);
    console.log('\nDone!');
  } catch (error) {
    if (error instanceof Error) {
      console.error(`\n✗ Error: ${error.message}`);
      if (error instanceof z.ZodError) {
        console.error('Validation details:', z.flattenError(error));
      }
    } else {
      console.error('\n✗ An unknown error occurred:', error);
    }
    process.exit(1);
  }
}

void main();

import pRetry from 'p-retry';
import OpenAI from 'openai';
import chalk from 'chalk';
import { z } from 'zod';
import { writeFile, appendFile } from 'fs/promises';
import { parseLlmJson } from '../utils/parse-llm-json.js';
import { calculateCost } from '../utils/llm-cost.js';
import { fillTemplate } from '../batch-processing/util.js';
import { Spinner } from '../utils/spinner.js';
import type { Config } from '../config.js';
import type { CardCandidate } from '../types.js';

export interface CostInfo {
  inputTokens: number;
  outputTokens: number;
  totalCost: number;
}

export interface GenerationResult {
  successful: CardCandidate[];
  failed: Array<{ prompt: string; error: Error }>;
  costInfo?: CostInfo;
}

/**
 * Logs a message to the specified log file.
 */
async function logToFile(
  logFilePath: string | undefined,
  message: string,
): Promise<void> {
  if (!logFilePath) return;

  try {
    const timestamp = new Date().toISOString();
    const logEntry = `[${timestamp}] ${message}\n`;
    await appendFile(logFilePath, logEntry, 'utf-8');
  } catch (error) {
    // Don't let log failures crash the program
    const errorMsg = error instanceof Error ? error.message : String(error);
    console.error(chalk.red(`Failed to write to log file: ${errorMsg}`));
  }
}

/**
 * Generates multiple flashcard candidates by making a single LLM API call.
 *
 * This processor is lightweight and purpose-built for the generate command,
 * separate from the batch processing system.
 *
 * @param term - The term/word to generate cards for
 * @param promptTemplate - Template with {term} and {count} placeholders
 * @param count - Number of cards to generate
 * @param config - Application configuration
 * @param fieldMap - Mapping from LLM JSON keys to expected field names
 * @param logFilePath - Optional path to log file for LLM responses
 * @returns Results with successful cards and failed attempts
 */
export async function generateCards(
  term: string,
  promptTemplate: string,
  count: number,
  config: Config,
  fieldMap: Record<string, string>,
  logFilePath?: string,
): Promise<GenerationResult> {
  // 1. Validate template contract
  if (
    !promptTemplate.includes('{term}') ||
    !promptTemplate.includes('{count}')
  ) {
    throw new Error(
      'Prompt template is invalid. It must include both {term} and {count} placeholders.',
    );
  }

  // 2. Create a Zod schema for an array of card objects
  const CardObjectSchema = z.object(
    Object.keys(fieldMap).reduce(
      (acc, key) => {
        acc[key] = z.string();
        return acc;
      },
      {} as Record<string, z.ZodString>,
    ),
  );
  const CardArraySchema = z.array(CardObjectSchema);

  // 3. Initialize log file if needed
  if (logFilePath) {
    await writeFile(logFilePath, '', 'utf-8');
    await logToFile(logFilePath, '='.repeat(60));
    await logToFile(logFilePath, `Generate Session Started`);
    await logToFile(logFilePath, `Term: ${term}`);
    await logToFile(logFilePath, `Count: ${count}`);
    await logToFile(logFilePath, '='.repeat(60));
  }

  // 4. Fill the template with both term and count
  const filledPrompt = fillTemplate(promptTemplate, {
    term,
    count: String(count),
  });

  if (logFilePath) {
    await logToFile(logFilePath, '\n--- PROMPT ---');
    await logToFile(logFilePath, filledPrompt);
    await logToFile(logFilePath, '--- END PROMPT ---\n');
  }

  const client = new OpenAI({
    apiKey: config.apiKey,
    baseURL: config.apiBaseUrl,
  });

  console.log(
    chalk.cyan(
      `\n🔄 Generating ${count} card candidate${count === 1 ? '' : 's'} for "${term}" using ${config.model}...`,
    ),
  );

  if (logFilePath) {
    console.log(chalk.gray(`📝 Logging to: ${logFilePath}`));
  }

  const generationSpinner = new Spinner();
  generationSpinner.start(`Waiting for ${config.model}...`);

  try {
    // 5. Perform a single, retry-able API call
    const { cards, rawResponse, costInfo } = await pRetry(
      async () => {
        const response = await client.chat.completions.create({
          model: config.model,
          messages: [{ role: 'user', content: filledPrompt }],
          temperature: config.temperature,
          ...(config.maxTokens && { max_tokens: config.maxTokens }),
        });

        const usage = response.usage;
        const inputTokens = usage?.prompt_tokens ?? 0;
        const outputTokens = usage?.completion_tokens ?? 0;

        if (!usage) {
          generationSpinner.interrupt(() => {
            console.warn(
              chalk.yellow(
                '  ⚠️  Could not determine token usage from API response. Cost will not be calculated.',
              ),
            );
          });
        }

        const totalCost = calculateCost(
          config.model,
          inputTokens,
          outputTokens,
        );
        const costInfo: CostInfo = { inputTokens, outputTokens, totalCost };

        const rawContent = response.choices[0]?.message?.content?.trim() || '';

        // Log the raw response (even if it's empty or will fail parsing)
        if (logFilePath) {
          await logToFile(logFilePath, '\n--- RAW RESPONSE ---');
          await logToFile(logFilePath, rawContent || '(empty response)');
          await logToFile(logFilePath, '--- END RAW RESPONSE ---\n');
        }

        if (!rawContent) {
          throw new Error('Empty response from LLM');
        }

        const parsed = parseLlmJson(rawContent);
        if (!Array.isArray(parsed)) {
          throw new Error(
            `Expected a JSON array, but got ${typeof parsed}: ${JSON.stringify(
              parsed,
            )}`,
          );
        }

        const validatedCards = CardArraySchema.parse(parsed);
        return { cards: validatedCards, rawResponse: rawContent, costInfo };
      },
      {
        retries: config.retries,
        minTimeout: 1000,
        factor: 2,
        onFailedAttempt: async ({ error, attemptNumber, retriesLeft }) => {
          const errorMsg = error.message || String(error);
          generationSpinner.interrupt(() => {
            console.warn(
              chalk.yellow(
                `  ⚠️  Attempt ${attemptNumber} failed. ${retriesLeft} retries left. Reason: ${errorMsg}`,
              ),
            );
          });

          if (retriesLeft > 0) {
            generationSpinner.update(
              `Retrying with ${retriesLeft} more attempt${
                retriesLeft === 1 ? '' : 's'
              }...`,
            );
          }

          // Log to file
          if (logFilePath) {
            await logToFile(
              logFilePath,
              `\n--- RETRY ATTEMPT ${attemptNumber} FAILED ---`,
            );
            await logToFile(logFilePath, `Retries left: ${retriesLeft}`);
            await logToFile(logFilePath, `Error: ${errorMsg}`);
            if (error.stack) {
              await logToFile(logFilePath, `Stack: ${error.stack}`);
            }
            await logToFile(
              logFilePath,
              `--- END RETRY ATTEMPT ${attemptNumber} ---\n`,
            );
          }
        },
      },
    );

    generationSpinner.succeed('LLM response received');

    // 5. Handle potential count mismatch
    if (cards.length !== count) {
      console.log(
        chalk.yellow(
          `  ⚠️  Warning: Requested ${count} cards, but received ${cards.length}.`,
        ),
      );
    }

    const successful: CardCandidate[] = cards.map((cardFields) => ({
      fields: cardFields,
      rawResponse,
    }));

    console.log(
      chalk.cyan(
        `✓ Generation complete: ${successful.length} succeeded, 0 failed`,
      ),
    );

    return { successful, failed: [], costInfo };
  } catch (error) {
    generationSpinner.fail('LLM generation failed');
    let err: Error;
    if (error instanceof Error) {
      err = error;
    } else if (typeof error === 'object' && error !== null) {
      // Try to extract useful info from objects (like API errors)
      err = new Error(JSON.stringify(error, null, 2));
    } else {
      err = new Error(String(error));
    }
    console.error(chalk.red(`\n✗ Generation failed: ${err.message}`));

    // Log the error
    if (logFilePath) {
      await logToFile(logFilePath, '\n--- ERROR ---');
      await logToFile(logFilePath, err.message);
      if (err.stack) {
        await logToFile(logFilePath, err.stack);
      }
      await logToFile(logFilePath, '--- END ERROR ---\n');
    }

    return {
      successful: [],
      failed: [{ prompt: filledPrompt, error: err }],
    };
  }
}

import pLimit from 'p-limit';
import pRetry from 'p-retry';
import OpenAI from 'openai';
import chalk from 'chalk';
import { z } from 'zod';
import { parseLlmJson, isObject } from '../utils/parse-llm-json.js';
import { fillTemplate } from '../batch-processing/util.js';
import type { Config } from '../config.js';

export interface CardCandidate {
  fields: Record<string, string>;
  rawResponse: string;
}

export interface GenerationResult {
  successful: CardCandidate[];
  failed: Array<{ prompt: string; error: Error }>;
}

/**
 * Generates multiple flashcard candidates by making parallel LLM API calls.
 *
 * This processor is lightweight and purpose-built for the generate command,
 * separate from the batch processing system.
 *
 * @param term - The term/word to generate cards for
 * @param promptTemplate - Template with {term} placeholder
 * @param count - Number of cards to generate
 * @param config - Application configuration
 * @param fieldMap - Mapping from LLM JSON keys to expected field names
 * @returns Results with successful cards and failed attempts
 */
export async function generateCards(
  term: string,
  promptTemplate: string,
  count: number,
  config: Config,
  fieldMap: Record<string, string>,
): Promise<GenerationResult> {
  // Create a Zod schema dynamically based on the fieldMap keys
  // All fields must be strings as per the plan
  const CardCandidateSchema = z.object(
    Object.keys(fieldMap).reduce(
      (acc, key) => {
        acc[key] = z.string();
        return acc;
      },
      {} as Record<string, z.ZodString>,
    ),
  );

  // Fill the template with the term
  const filledPrompt = fillTemplate(promptTemplate, { term });

  // Create an array of identical prompts (one for each card to generate)
  const prompts = Array.from({ length: count }, () => filledPrompt);

  // Initialize OpenAI client
  const client = new OpenAI({
    apiKey: config.apiKey,
    baseURL: config.apiBaseUrl,
  });

  // Create a concurrency limiter
  const limit = pLimit(config.batchSize);

  console.log(
    chalk.cyan(
      `\n🔄 Generating ${count} card${count === 1 ? '' : 's'} for "${term}"...\n`,
    ),
  );

  // Process all prompts in parallel with retry logic
  const results = await Promise.all(
    prompts.map((prompt, index) =>
      limit(async () => {
        try {
          const card = await pRetry(
            async () => {
              // Make API call
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

              const rawResponse =
                response.choices[0]?.message?.content?.trim() || '';

              if (!rawResponse) {
                throw new Error('Empty response from LLM');
              }

              // Robustly parse JSON from the response
              const parsed = parseLlmJson(rawResponse);

              // Verify it's an object (not array, string, etc.)
              if (!isObject(parsed)) {
                throw new Error(
                  `Expected JSON object, got ${typeof parsed}: ${JSON.stringify(parsed)}`,
                );
              }

              // Validate with Zod schema
              const validatedFields = CardCandidateSchema.parse(parsed);

              return {
                fields: validatedFields,
                rawResponse,
              };
            },
            {
              retries: config.retries,
              minTimeout: 1000, // 1 second
              factor: 2, // Exponential backoff
              onFailedAttempt: (error) => {
                console.warn(
                  chalk.yellow(
                    `  ⚠️  Card ${index + 1}: Attempt ${error.attemptNumber} failed. ${error.retriesLeft} retries left.`,
                  ),
                );
              },
            },
          );

          console.log(chalk.green(`  ✓ Card ${index + 1} generated`));
          return { success: true as const, card };
        } catch (error) {
          const err = error instanceof Error ? error : new Error(String(error));
          console.error(
            chalk.red(`  ✗ Card ${index + 1} failed: ${err.message}`),
          );
          return { success: false as const, prompt, error: err };
        }
      }),
    ),
  );

  // Separate successful and failed results
  const successful = results
    .filter((r): r is { success: true; card: CardCandidate } => r.success)
    .map((r) => r.card);

  const failed = results
    .filter(
      (r): r is { success: false; prompt: string; error: Error } => !r.success,
    )
    .map((r) => ({ prompt: r.prompt, error: r.error }));

  console.log(
    chalk.cyan(
      `\n✓ Generation complete: ${successful.length} succeeded, ${failed.length} failed\n`,
    ),
  );

  return { successful, failed };
}

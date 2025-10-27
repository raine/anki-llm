import pRetry from 'p-retry';
import OpenAI from 'openai';
import chalk from 'chalk';
import { z } from 'zod';
import { parseLlmJson } from '../utils/parse-llm-json.js';
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
 * @returns Results with successful cards and failed attempts
 */
export async function generateCards(
  term: string,
  promptTemplate: string,
  count: number,
  config: Config,
  fieldMap: Record<string, string>,
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

  // 3. Fill the template with both term and count
  const filledPrompt = fillTemplate(promptTemplate, {
    term,
    count: String(count),
  });

  const client = new OpenAI({
    apiKey: config.apiKey,
    baseURL: config.apiBaseUrl,
  });

  console.log(
    chalk.cyan(
      `\n🔄 Generating ${count} card candidate${count === 1 ? '' : 's'} for "${term}"...`,
    ),
  );

  try {
    // 4. Perform a single, retry-able API call
    const { cards, rawResponse } = await pRetry(
      async () => {
        const response = await client.chat.completions.create({
          model: config.model,
          messages: [{ role: 'user', content: filledPrompt }],
          temperature: config.temperature,
          ...(config.maxTokens && { max_tokens: config.maxTokens }),
        });

        const rawContent = response.choices[0]?.message?.content?.trim() || '';
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
        return { cards: validatedCards, rawResponse: rawContent };
      },
      {
        retries: config.retries,
        minTimeout: 1000,
        factor: 2,
        onFailedAttempt: (error) => {
          const errorMsg =
            error instanceof Error ? error.message : 'Unknown error';
          console.warn(
            chalk.yellow(
              `  ⚠️  Attempt ${error.attemptNumber} failed. ${error.retriesLeft} retries left. Reason: ${errorMsg}`,
            ),
          );
        },
      },
    );

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
        `\n✓ Generation complete: ${successful.length} succeeded, 0 failed\n`,
      ),
    );

    return { successful, failed: [] };
  } catch (error) {
    const err = error instanceof Error ? error : new Error(String(error));
    console.error(chalk.red(`\n✗ Generation failed: ${err.message}`));

    return {
      successful: [],
      failed: [{ prompt: filledPrompt, error: err }],
    };
  }
}

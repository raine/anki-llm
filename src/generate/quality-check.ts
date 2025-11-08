import chalk from 'chalk';
import { confirm } from '@inquirer/prompts';
import pRetry from 'p-retry';
import OpenAI from 'openai';
import { z } from 'zod';

import type { Config } from '../config.js';
import { SupportedModel } from '../config.js';
import type { ValidatedCard } from '../types.js';
import type { Frontmatter } from '../utils/parse-frontmatter.js';
import { parseLlmJson } from '../utils/parse-llm-json.js';
import { calculateCost, formatCost } from '../utils/llm-cost.js';

// Schema for the expected JSON response from the quality check LLM call.
const QualityCheckResponse = z.object({
  is_valid: z.boolean(),
  reason: z.string(),
});
type QualityCheckResponse = z.infer<typeof QualityCheckResponse>;

interface CheckResult {
  card: ValidatedCard;
  isValid: boolean;
  reason: string;
  cost: number;
}

/**
 * Checks a single card for quality.
 */
async function checkCard(
  card: ValidatedCard,
  checkConfig: NonNullable<Frontmatter['qualityCheck']>,
  appConfig: Config,
  client: OpenAI,
): Promise<QualityCheckResponse & { cost: number }> {
  const textToCheck = card.fields[checkConfig.field];
  if (!textToCheck) {
    throw new Error(`Field "${checkConfig.field}" not found on card.`);
  }

  const filledPrompt = checkConfig.prompt.replace('{text}', textToCheck);

  // Use the quality-check specific model if provided, otherwise fallback
  const model = checkConfig.model ?? appConfig.model;

  const response = await client.chat.completions.create({
    model,
    messages: [{ role: 'user', content: filledPrompt }],
    temperature: appConfig.temperature,
    response_format: { type: 'json_object' },
    ...(appConfig.maxTokens && { max_tokens: appConfig.maxTokens }),
  });

  const rawContent = response.choices[0]?.message?.content?.trim() || '';
  if (!rawContent) {
    throw new Error('Empty response from LLM');
  }

  const parsed = parseLlmJson(rawContent);
  const validatedResponse = QualityCheckResponse.parse(parsed);

  const usage = response.usage;
  const inputTokens = usage?.prompt_tokens ?? 0;
  const outputTokens = usage?.completion_tokens ?? 0;

  // Validate model before calculating cost
  let cost = 0;
  const modelResult = SupportedModel.safeParse(model);
  if (modelResult.success) {
    cost = calculateCost(modelResult.data, inputTokens, outputTokens);
  }

  return { ...validatedResponse, cost };
}

/**
 * Performs an optional quality check on selected cards.
 * This involves additional LLM calls and an interactive review process for flagged cards.
 */
export async function performQualityCheck(
  selectedCards: ValidatedCard[],
  frontmatter: Frontmatter,
  appConfig: Config,
): Promise<{ finalCards: ValidatedCard[]; cost: number }> {
  const checkConfig = frontmatter.qualityCheck;
  if (!checkConfig) {
    return { finalCards: selectedCards, cost: 0 };
  }

  const model = checkConfig.model ?? appConfig.model;

  console.log(
    chalk.cyan(
      `\n🔬 Running quality check on ${selectedCards.length} selected card(s) using ${model}...`,
    ),
  );

  const client = new OpenAI({
    apiKey: appConfig.apiKey,
    baseURL: appConfig.apiBaseUrl,
  });

  const checkPromises = selectedCards.map(async (card) => {
    try {
      const result = await pRetry(
        () => checkCard(card, checkConfig, appConfig, client),
        {
          retries: appConfig.retries,
          onFailedAttempt: (error) => {
            console.warn(
              chalk.yellow(
                `  ⚠️  Attempt ${error.attemptNumber} failed for quality check. ${error.retriesLeft} retries left.`,
              ),
            );
          },
        },
      );
      return {
        card,
        isValid: result.is_valid,
        reason: result.reason,
        cost: result.cost,
        error: null,
      };
    } catch (error) {
      const msg = error instanceof Error ? error.message : String(error);
      console.error(chalk.red(`\n❌ Failed to check card quality: ${msg}`));
      console.log(chalk.gray('  Skipping check for this card.\n'));
      return { card, isValid: true, reason: 'Check failed', cost: 0, error };
    }
  });

  const results: Array<CheckResult & { error: unknown }> =
    await Promise.all(checkPromises);
  const totalCost = results.reduce((sum, r) => sum + r.cost, 0);

  if (totalCost > 0) {
    console.log(chalk.gray(`  Quality check cost: ${formatCost(totalCost)}`));
  }

  const flagged = results.filter((r) => !r.isValid);

  if (flagged.length === 0) {
    console.log(chalk.green('\n✓ All cards passed the quality check.'));
    return { finalCards: selectedCards, cost: totalCost };
  }

  console.log(
    chalk.yellow(
      `\n⚠️  ${flagged.length} card(s) were flagged by the quality check. Please review:`,
    ),
  );

  const cardsToKeep = new Set<ValidatedCard>(
    results.filter((r) => r.isValid).map((r) => r.card),
  );

  for (const [index, item] of flagged.entries()) {
    console.log(
      chalk.cyan(`\n--- Flagged Card ${index + 1}/${flagged.length} ---`),
    );
    Object.entries(item.card.fields).forEach(([key, value]) => {
      console.log(`${chalk.bold(key)}: ${value}`);
    });
    console.log(chalk.yellow(`\nReason: ${item.reason}`));

    const keep = await confirm({
      message: 'Keep this card anyway?',
      default: false,
    });

    if (keep) {
      cardsToKeep.add(item.card);
    }
  }

  const finalCards = selectedCards.filter((card) => cardsToKeep.has(card));

  return { finalCards, cost: totalCost };
}

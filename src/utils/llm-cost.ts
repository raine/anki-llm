import chalk from 'chalk';
import { MODEL_PRICING, type SupportedChatModel } from '../config.js';

/**
 * Calculate cost from token usage and model pricing
 */
export function calculateCost(
  model: SupportedChatModel,
  inputTokens: number,
  outputTokens: number,
): number {
  const pricing = MODEL_PRICING[model];
  if (!pricing) {
    // Fallback if pricing data is missing for a model
    return 0;
  }
  const inputCost = (inputTokens / 1_000_000) * pricing.inputCostPerMillion;
  const outputCost = (outputTokens / 1_000_000) * pricing.outputCostPerMillion;
  return inputCost + outputCost;
}

/**
 * Formats a cost value (in USD) for display.
 * Always shows dollars for consistency and easier visual parsing.
 */
export function formatCost(cost: number): string {
  return `$${cost.toFixed(4)}`;
}

/**
 * Formats and displays cost information with token counts.
 */
export function formatCostDisplay(
  totalCost: number,
  inputTokens: number,
  outputTokens: number,
): string {
  return chalk.gray(
    `  Cost: ${formatCost(totalCost)} (${inputTokens} input + ${outputTokens} output tokens)`,
  );
}

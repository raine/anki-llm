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
 * Uses cents for values under $0.01.
 */
export function formatCost(cost: number): string {
  if (cost === 0) return '$0.00';
  if (cost < 0.01) {
    // Show in cents for small amounts
    const cents = cost * 100;
    return `${cents.toFixed(1)}¢`;
  }
  // Show in dollars, with enough precision for API costs
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

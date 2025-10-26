import type { SupportedChatModel } from '../config.js';

// Generic row data type - can hold any fields
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export type RowData = Record<string, any>;

/**
 * Enhanced row with error tracking
 */
export type ProcessedRow = RowData & { _error?: string };

/**
 * Token usage statistics
 */
export type TokenStats = {
  input: number;
  output: number;
};

/**
 * Model pricing information
 */
export type ModelPricing = {
  inputCostPerMillion: number;
  outputCostPerMillion: number;
};

/**
 * Pricing data for all supported models
 */
export const MODEL_PRICING: Record<SupportedChatModel, ModelPricing> = {
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
  'gemini-2.5-pro': {
    inputCostPerMillion: 1.25,
    outputCostPerMillion: 10,
  },
};

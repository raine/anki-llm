import { describe, it, expect } from 'vitest';
import { formatCost, calculateCost, formatCostDisplay } from './llm-cost.js';

describe('formatCost', () => {
  it('formats zero cost', () => {
    expect(formatCost(0)).toBe('$0.0000');
  });

  it('formats small costs in dollars', () => {
    expect(formatCost(0.001)).toBe('$0.0010');
    expect(formatCost(0.005)).toBe('$0.0050');
    expect(formatCost(0.008)).toBe('$0.0080');
    expect(formatCost(0.0001)).toBe('$0.0001');
    expect(formatCost(0.00001)).toBe('$0.0000'); // rounds to 4 decimals
  });

  it('formats costs with 4 decimal places', () => {
    expect(formatCost(0.01)).toBe('$0.0100');
    expect(formatCost(0.0255)).toBe('$0.0255');
    expect(formatCost(0.1)).toBe('$0.1000');
    expect(formatCost(1.0)).toBe('$1.0000');
    expect(formatCost(10.5678)).toBe('$10.5678');
  });

  it('rounds to 4 decimal places', () => {
    expect(formatCost(0.012345)).toBe('$0.0123');
    expect(formatCost(0.012355)).toBe('$0.0124');
  });
});

describe('calculateCost', () => {
  it('calculates cost for GPT-5-mini', () => {
    // GPT-5-mini: $0.25/M input, $2/M output
    const cost = calculateCost('gpt-5-mini', 1000, 500);
    // (1000/1M * 0.25) + (500/1M * 2) = 0.00025 + 0.001 = 0.00125
    expect(cost).toBeCloseTo(0.00125);
  });

  it('calculates cost for Gemini 2.5 Flash', () => {
    // Gemini 2.5 Flash: $0.3/M input, $2.5/M output
    const cost = calculateCost('gemini-2.5-flash', 10000, 5000);
    // (10000/1M * 0.3) + (5000/1M * 2.5) = 0.003 + 0.0125 = 0.0155
    expect(cost).toBeCloseTo(0.0155);
  });

  it('handles zero tokens', () => {
    const cost = calculateCost('gpt-5-mini', 0, 0);
    expect(cost).toBe(0);
  });

  it('handles large token counts', () => {
    // GPT-5-mini with 1 million input and output tokens
    const cost = calculateCost('gpt-5-mini', 1_000_000, 1_000_000);
    // (1M/1M * 0.25) + (1M/1M * 2) = 0.25 + 2 = 2.25
    expect(cost).toBeCloseTo(2.25);
  });

  it('handles input tokens only', () => {
    // GPT-5-mini: $0.25/M input
    const cost = calculateCost('gpt-5-mini', 100_000, 0);
    // (100000/1M * 0.25) = 0.025
    expect(cost).toBeCloseTo(0.025);
  });

  it('handles output tokens only', () => {
    // GPT-5-mini: $2/M output
    const cost = calculateCost('gpt-5-mini', 0, 100_000);
    // (100000/1M * 2) = 0.2
    expect(cost).toBeCloseTo(0.2);
  });

  it('returns 0 for an unknown model', () => {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any, @typescript-eslint/no-unsafe-argument
    const cost = calculateCost('unknown-model' as any, 1000, 1000);
    expect(cost).toBe(0);
  });
});

describe('formatCostDisplay', () => {
  it('formats cost and token counts for a dollar amount', () => {
    const displayString = formatCostDisplay({
      totalCost: 0.1234,
      inputTokens: 1000,
      outputTokens: 500,
    });
    expect(displayString).toContain('Cost: $0.1234');
    expect(displayString).toContain('(1000 input + 500 output tokens)');
  });

  it('formats cost and token counts for small amounts', () => {
    const displayString = formatCostDisplay({
      totalCost: 0.005,
      inputTokens: 50,
      outputTokens: 20,
    });
    expect(displayString).toContain('Cost: $0.0050');
    expect(displayString).toContain('(50 input + 20 output tokens)');
  });

  it('handles zero cost and tokens', () => {
    const displayString = formatCostDisplay({
      totalCost: 0,
      inputTokens: 0,
      outputTokens: 0,
    });
    expect(displayString).toContain('Cost: $0.0000');
    expect(displayString).toContain('(0 input + 0 output tokens)');
  });
});

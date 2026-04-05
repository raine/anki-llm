import { jsonrepair } from 'jsonrepair';

/**
 * Robustly parses JSON from LLM responses that may contain extra text or formatting.
 *
 * Uses jsonrepair to handle common LLM output issues:
 * - Markdown code fences (```json ... ```)
 * - Truncated or incomplete JSON
 * - Missing quotes, commas, or brackets
 * - Trailing commas
 * - Comments
 * - Extra explanatory text before/after the JSON
 *
 * @param text - Raw text from LLM response
 * @returns Parsed JSON data (object, array, etc.)
 * @throws Error if no valid JSON can be extracted
 */
export function parseLlmJson(text: string): unknown {
  try {
    const repairedJson = jsonrepair(text);
    return JSON.parse(repairedJson) as unknown;
  } catch (error) {
    const message = error instanceof Error ? error.message : 'Unknown error';
    throw new Error(
      `Failed to repair and parse JSON: ${message}\n\n` +
        `Input text: ${text.substring(0, 700)}...`,
    );
  }
}

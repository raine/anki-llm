/**
 * Robustly parses JSON from LLM responses that may contain extra text or formatting.
 *
 * This utility handles common LLM output issues:
 * - Markdown code fences (```json ... ```)
 * - Extra explanatory text before/after the JSON
 * - Inconsistent whitespace
 *
 * @param text - Raw text from LLM response
 * @returns Parsed JSON object
 * @throws Error if no valid JSON object can be extracted
 */
export function parseLlmJson(text: string): unknown {
  // Step 1: Check for markdown code block
  const markdownMatch = text.match(/```(?:json)?\s*([\s\S]+?)\s*```/);
  if (markdownMatch && markdownMatch[1]) {
    text = markdownMatch[1];
  }

  // Step 2: Find the first '{' and last '}'
  const firstBrace = text.indexOf('{');
  const lastBrace = text.lastIndexOf('}');

  if (firstBrace === -1 || lastBrace === -1 || lastBrace < firstBrace) {
    throw new Error(
      'Could not find a valid JSON object in the response. ' +
        'The response should contain a JSON object enclosed in curly braces {}.\n\n' +
        `Response preview: ${text.substring(0, 200)}${text.length > 200 ? '...' : ''}`,
    );
  }

  // Extract the JSON substring
  const jsonText = text.substring(firstBrace, lastBrace + 1);

  // Step 3: Attempt to parse
  try {
    return JSON.parse(jsonText) as unknown;
  } catch (error) {
    const message = error instanceof Error ? error.message : 'Unknown error';
    throw new Error(
      `Failed to parse extracted JSON: ${message}\n\n` +
        `Extracted text: ${jsonText.substring(0, 200)}${jsonText.length > 200 ? '...' : ''}`,
    );
  }
}

/**
 * Type guard to check if the parsed value is a non-null object.
 * Useful for validating that parseLlmJson returned an object (not array, string, etc.)
 */
export function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

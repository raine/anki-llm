import { AbortError } from 'p-retry';
import type { RowData } from './types.js';

/**
 * Extracts noteId from a row, ensuring it's a valid string or number.
 * Checks multiple possible field names: noteId, id, Id
 * Returns undefined only during validation. After validation, all rows are guaranteed to have an ID.
 * Note: Always normalizes to string to avoid Map key mismatch between '123' and 123.
 */
export function getNoteId(row: RowData): string | undefined {
  // Check each possible field name in order
  // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
  const noteId = row.noteId ?? row.id ?? row.Id;

  // Ensure the value is actually a string or number (not an object, array, etc.)
  if (typeof noteId === 'string' || typeof noteId === 'number') {
    // Normalize to string to prevent Map key mismatch ('123' vs 123)
    return String(noteId);
  }

  // No valid identifier found, or the value is an unexpected type
  return undefined;
}

/**
 * Same as getNoteId but throws if no ID is found.
 * Use this after validation when all rows are guaranteed to have IDs.
 */
export function requireNoteId(row: RowData): string {
  const noteId = getNoteId(row);
  if (noteId === undefined) {
    throw new Error(
      `Row missing required identifier (noteId, id, or Id). This should not happen after validation. Fields: ${Object.keys(row).join(', ')}`,
    );
  }
  return noteId;
}

/**
 * Fills a template string with data from a row object with robust error handling.
 *
 * This function provides the following guarantees:
 * 1.  **Strictness**: Throws an error if any placeholder in the template does not have a
 *     corresponding key in the data object.
 * 2.  **Case-Insensitivity**: Matches placeholders like `{FieldName}` or `{fieldname}` to
 *     data keys like `fieldName` or `FieldName`.
 * 3.  **Safety**: Detects and throws an error for ambiguous keys in the source data
 *     (e.g., a row with both a 'name' and 'Name' property).
 * 4.  **Efficiency**: Uses a single-pass regex replacement, which is more performant
 *     than iterative methods.
 *
 * @param template The template string with placeholders in `{key}` format.
 * @param row The data object providing values for the placeholders.
 * @returns The processed string with all placeholders replaced.
 * @throws {Error} if the row data contains ambiguous keys (e.g., 'name' and 'Name').
 * @throws {Error} if the template contains placeholders that are not found in the row data.
 */
export function fillTemplate(template: string, row: RowData): string {
  // 1. Create a case-insensitive map of the row data to handle variations
  // in key casing (e.g., 'Email' vs. 'email') and detect ambiguities.
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const lowerCaseRow = new Map<string, any>();
  for (const [key, value] of Object.entries(row)) {
    const lowerKey = key.toLowerCase();
    if (lowerCaseRow.has(lowerKey)) {
      // Fail fast on ambiguous keys to prevent unpredictable behavior.
      // Use AbortError to prevent retries on configuration errors
      throw new AbortError(
        `Ambiguous key in row data: "${key}" conflicts with another key when case is ignored.`,
      );
    }
    lowerCaseRow.set(lowerKey, value);
  }

  // 2. Use a regex to find all unique placeholders required by the template.
  const placeholders = [...template.matchAll(/\{([^}]+)\}/g)];
  const requiredKeys = new Set(
    placeholders.map((match) => match[1].toLowerCase()),
  );

  // 3. Validate that all required placeholders exist in the data.
  // This is a critical check to prevent sending incomplete prompts to the LLM.
  const missingKeys: string[] = [];
  for (const key of requiredKeys) {
    if (!lowerCaseRow.has(key)) {
      // Find the original placeholder casing for a more helpful error message.
      const originalPlaceholder = placeholders.find(
        (p) => p[1].toLowerCase() === key,
      )?.[0];
      missingKeys.push(originalPlaceholder || `{${key}}`);
    }
  }

  if (missingKeys.length > 0) {
    // Use AbortError to prevent retries on configuration errors
    throw new AbortError(
      `Missing data for template placeholders: ${missingKeys.join(', ')}. Available fields: ${Object.keys(row).join(', ')}`,
    );
  }

  // 4. Perform the replacement in a single, efficient pass.
  return template.replace(/\{([^}]+)\}/g, (_match, key: string) => {
    const lowerKey = key.toLowerCase();
    // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
    const value = lowerCaseRow.get(lowerKey);
    // Coerce null/undefined to an empty string, preserving the original intent.
    return String(value ?? '');
  });
}

/**
 * Removes ANSI escape codes (used for colors) from a string.
 */
export function stripAnsi(str: string): string {
  // eslint-disable-next-line no-control-regex
  return str.replace(/\x1b\[[0-9;]*m/g, '');
}

/**
 * Wraps a promise with a timeout. If the promise doesn't resolve within the timeout,
 * it rejects with a timeout error.
 */
export function withTimeout<T>(
  promise: Promise<T>,
  timeoutMs: number,
  errorMessage: string,
): Promise<T> {
  return Promise.race([
    promise,
    new Promise<T>((_, reject) =>
      setTimeout(() => reject(new Error(errorMessage)), timeoutMs),
    ),
  ]);
}

/**
 * Tries to parse a string as JSON.
 * Returns a Record<string, any> if the string is a valid JSON object.
 * Returns null if parsing fails or if the result is not a plain object (e.g., an array).
 */
export function tryParseJsonObject(
  jsonString: string,
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
): Record<string, any> | null {
  try {
    // Return null for empty or non-string inputs
    if (!jsonString || typeof jsonString !== 'string') {
      return null;
    }

    // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
    const parsed = JSON.parse(jsonString);

    // Ensure the parsed result is a non-null, non-array object
    if (
      typeof parsed === 'object' &&
      parsed !== null &&
      !Array.isArray(parsed)
    ) {
      // eslint-disable-next-line @typescript-eslint/no-unsafe-return
      return parsed;
    }

    // The JSON is valid but not a mergeable object (e.g., it's an array or primitive)
    return null;
  } catch {
    // The string is not valid JSON
    return null;
  }
}

/**
 * Merges fields from a source object into a target row using case-insensitive matching.
 *
 * This function ensures consistency with fillTemplate, which reads fields case-insensitively.
 * It prevents field duplication that would occur with a simple spread operation.
 *
 * Behavior:
 * - If a field in the source matches an existing field in the target (case-insensitively),
 *   the target's field is updated while preserving its original casing.
 * - If a field in the source is new (no case-insensitive match), it's added as-is.
 * - Throws an error if the target row contains ambiguous keys (e.g., both "Name" and "name").
 *
 * @param targetRow The row object to merge into (typically the existing note data).
 * @param sourceFields The object containing fields to merge (typically from LLM JSON response).
 * @returns A new object with merged fields.
 * @throws {Error} if the target row has ambiguous keys that match when case is ignored.
 *
 * @example
 * const note = { noteId: "123", Translation: "old", Japanese: "古い" };
 * const llmResponse = { translation: "new", grammar: "noun" };
 * const result = mergeFieldsCaseInsensitive(note, llmResponse);
 * // Returns: { noteId: "123", Translation: "new", Japanese: "古い", grammar: "noun" }
 * //           Note: "Translation" was updated (not duplicated as "translation")
 */
export function mergeFieldsCaseInsensitive(
  targetRow: RowData,
  sourceFields: RowData,
): RowData {
  const resultRow = { ...targetRow };

  // Create a lookup from lowercase key -> original key, checking for ambiguity
  const keyMap = new Map<string, string>();
  for (const key of Object.keys(resultRow)) {
    const lowerKey = key.toLowerCase();
    if (keyMap.has(lowerKey)) {
      throw new Error(
        `Ambiguous key in row data: "${key}" and "${keyMap.get(lowerKey)}" are the same when case is ignored.`,
      );
    }
    keyMap.set(lowerKey, key);
  }

  // Merge fields from source using case-insensitive matching
  for (const [sourceKey, sourceValue] of Object.entries(sourceFields)) {
    const matchingKey = keyMap.get(sourceKey.toLowerCase());
    if (matchingKey) {
      // Update existing field, preserving original casing
      // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
      resultRow[matchingKey] = sourceValue;
    } else {
      // Add as a new field
      // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
      resultRow[sourceKey] = sourceValue;
    }
  }

  return resultRow;
}

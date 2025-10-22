import { z } from 'zod';

export const ANKI_CONNECT_URL = 'http://127.0.0.1:8765';

// Generic response schema
export function AnkiConnectResponse<T extends z.ZodTypeAny>(resultSchema: T) {
  return z.object({
    result: resultSchema.nullable(),
    error: z.string().nullable(),
  });
}

// Common schemas
export const NoteField = z.object({
  value: z.string(),
  order: z.number(),
});

export const NoteInfo = z.object({
  noteId: z.number(),
  fields: z.record(z.string(), NoteField.optional()),
  tags: z.array(z.string()),
  modelName: z.string(),
});

/**
 * Helper function to send requests to AnkiConnect with schema validation.
 */
export async function ankiRequest<
  R,
  P extends Record<string, unknown> = Record<string, never>,
>(action: string, resultSchema: z.ZodType<R>, params?: P): Promise<R> {
  const payload = { action, params: params ?? {}, version: 6 };

  try {
    const response = await fetch(ANKI_CONNECT_URL, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
    });

    if (!response.ok) {
      throw new Error(`HTTP error! status: ${response.status}`);
    }

    // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
    const responseJson = await response.json();
    const validatedResponse =
      AnkiConnectResponse(resultSchema).parse(responseJson);

    if (validatedResponse.error) {
      throw new Error(`AnkiConnect API error: ${validatedResponse.error}`);
    }
    if (validatedResponse.result === null) {
      throw new Error(`AnkiConnect returned null result for action: ${action}`);
    }
    return validatedResponse.result;
  } catch (error) {
    if (error instanceof z.ZodError) {
      console.error('Zod validation error:', z.flattenError(error));
      throw new Error('AnkiConnect response validation failed.');
    }
    if (error instanceof Error && error.message.includes('fetch')) {
      throw new Error(
        `Network error: Could not connect to AnkiConnect. Is Anki running? Details: ${error.message}`,
      );
    }
    throw error;
  }
}

import { z } from 'zod';

function buildFieldSchema() {
  return z.union([z.string(), z.array(z.string())]);
}

export function buildCardSchemas(fieldMap: Record<string, string>) {
  const schemaFields = Object.keys(fieldMap).reduce(
    (acc, key) => {
      acc[key] = buildFieldSchema();
      return acc;
    },
    {} as Record<string, ReturnType<typeof buildFieldSchema>>,
  );

  const cardObjectSchema = z.object(schemaFields);
  return {
    cardObjectSchema,
    cardArraySchema: z.array(cardObjectSchema),
  };
}

import { z } from 'zod';

const SupportedModelSchema = z.enum([
  'gpt-4.1',
  'gpt-4o',
  'gpt-4o-mini',
  'gpt-5-nano',
  'gemini-2.0-flash',
  'gemini-2.5-flash',
  'gemini-2.5-flash-lite-preview-06-17',
]);

export type SupportedChatModel = z.infer<typeof SupportedModelSchema>;

const Config = z.object({
  apiKey: z.string().min(1, 'API key is required'),
  apiBaseUrl: z.string().optional(),
  model: SupportedModelSchema,
  batchSize: z.number().int().positive(),
  dryRun: z.boolean(),
  maxTokens: z.number().int().positive().optional(),
  temperature: z.number().min(0).max(2),
  retries: z.number().int().min(0),
});

export type Config = z.infer<typeof Config>;

/**
 * Determines the provider and base URL based on the model name
 */
function getProviderConfig(model: SupportedChatModel): {
  baseURL?: string;
  recommendedApiKeyEnv: string;
} {
  if (model.startsWith('gpt-')) {
    return {
      recommendedApiKeyEnv: 'OPENAI_API_KEY',
    };
  } else if (model.startsWith('gemini-')) {
    return {
      baseURL: 'https://generativelanguage.googleapis.com/v1beta/openai/',
      recommendedApiKeyEnv: 'GEMINI_API_KEY',
    };
  }
  return {
    recommendedApiKeyEnv: 'OPENAI_API_KEY',
  };
}

export function parseConfig(): Config {
  const dryRun = process.env.DRY_RUN === 'true';

  // Get model first to determine which API key to look for
  const modelStr = process.env.MODEL || 'gpt-4o-mini';
  const modelResult = SupportedModelSchema.safeParse(modelStr);

  if (!modelResult.success) {
    console.error(`❌ Invalid MODEL: ${modelStr}`);
    console.error(
      `Supported models: ${SupportedModelSchema.options.join(', ')}`,
    );
    process.exit(1);
  }

  const model = modelResult.data;
  const providerConfig = getProviderConfig(model);

  // Check for API key
  const apiKey = process.env.OPENAI_API_KEY || process.env.GEMINI_API_KEY;

  // Skip API key check in dry run mode
  if (!dryRun && !apiKey) {
    console.error(
      '❌ Error: OPENAI_API_KEY or GEMINI_API_KEY environment variable is required',
    );
    console.error(
      `Tip: For model '${model}', set ${providerConfig.recommendedApiKeyEnv}`,
    );
    console.error('Or set DRY_RUN=true to preview without an API key');
    process.exit(1);
  }

  // Determine base URL: explicit override > provider default > undefined
  const apiBaseUrl =
    process.env.OPENAI_API_BASE ||
    process.env.API_BASE_URL ||
    providerConfig.baseURL;

  const result = Config.safeParse({
    apiKey: apiKey || 'dummy-key-for-dry-run',
    apiBaseUrl,
    model,
    batchSize: process.env.BATCH_SIZE
      ? parseInt(process.env.BATCH_SIZE, 10)
      : 5,
    dryRun,
    maxTokens: process.env.MAX_TOKENS
      ? parseInt(process.env.MAX_TOKENS, 10)
      : undefined,
    temperature: process.env.TEMPERATURE
      ? parseFloat(process.env.TEMPERATURE)
      : 0.3,
    retries: process.env.RETRIES ? parseInt(process.env.RETRIES, 10) : 3,
  });

  if (!result.success) {
    console.error('❌ Invalid configuration:');
    console.error(z.prettifyError(result.error));
    process.exit(1);
  }

  return result.data;
}

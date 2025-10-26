import { z } from 'zod';

export const SupportedModel = z.enum([
  'gpt-4.1',
  'gpt-4o',
  'gpt-4o-mini',
  'gpt-5-nano',
  'gemini-2.0-flash',
  'gemini-2.5-flash',
  'gemini-2.5-flash-lite',
  'gemini-2.5-pro',
]);

export type SupportedChatModel = z.infer<typeof SupportedModel>;

const Config = z.object({
  apiKey: z.string().min(1, 'API key is required'),
  apiBaseUrl: z.string().optional(),
  model: SupportedModel,
  batchSize: z.number().int().positive(),
  dryRun: z.boolean(),
  maxTokens: z.number().int().positive().optional(),
  temperature: z.number().min(0).max(2),
  retries: z.number().int().min(0),
  requireResultTag: z.boolean(),
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
      baseURL: 'https://generativelanguage.googleapis.com/v1beta/openai',
      recommendedApiKeyEnv: 'GEMINI_API_KEY',
    };
  }
  return {
    recommendedApiKeyEnv: 'OPENAI_API_KEY',
  };
}

export function parseConfig(cliArgs: {
  model: string;
  batchSize: number;
  maxTokens?: number;
  temperature: number;
  retries: number;
  dryRun: boolean;
  requireResultTag: boolean;
}): Config {
  const { model: modelStr, dryRun } = cliArgs;

  // Validate model
  const modelResult = SupportedModel.safeParse(modelStr);

  if (!modelResult.success) {
    console.error(`❌ Invalid MODEL: ${modelStr}`);
    console.error(`Supported models: ${SupportedModel.options.join(', ')}`);
    process.exit(1);
  }

  const model = modelResult.data;
  const providerConfig = getProviderConfig(model);

  // Check for API key based on the model's provider
  const apiKey = process.env[providerConfig.recommendedApiKeyEnv];

  // Skip API key check in dry run mode
  if (!dryRun && !apiKey) {
    console.error(
      '❌ Error: OPENAI_API_KEY or GEMINI_API_KEY environment variable is required',
    );
    console.error(
      `Tip: For model '${model}', set ${providerConfig.recommendedApiKeyEnv}`,
    );
    console.error('Or use --dry-run to preview without an API key');
    process.exit(1);
  }

  // Use provider-specific base URL
  const apiBaseUrl = providerConfig.baseURL;

  const result = Config.safeParse({
    apiKey: apiKey || 'dummy-key-for-dry-run',
    apiBaseUrl,
    model,
    batchSize: cliArgs.batchSize,
    dryRun,
    maxTokens: cliArgs.maxTokens,
    temperature: cliArgs.temperature,
    retries: cliArgs.retries,
    requireResultTag: cliArgs.requireResultTag,
  });

  if (!result.success) {
    console.error('❌ Invalid configuration:');
    console.error(z.prettifyError(result.error));
    process.exit(1);
  }

  return result.data;
}

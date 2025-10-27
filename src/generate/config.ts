import { parseConfig, SupportedModel, type Config } from '../config.js';
import { readConfigFile, type PersistentConfig } from '../config-manager.js';
import type { GenerationConfig } from '../generation/processor.js';

// Matches the yargs argv interface in generate.ts
interface GenerateCommandArgs {
  model?: string;
  'batch-size': number;
  'max-tokens'?: number;
  temperature: number;
  retries: number;
  'dry-run': boolean;
}

/**
 * Resolves the final model name from CLI args or user config.
 * Throws an error if no model is specified.
 */
function resolveModel(
  cliModel: string | undefined,
  userConfig: PersistentConfig,
): string {
  const model = cliModel ?? userConfig.model;

  if (!model) {
    throw new Error(
      `A model must be specified via the --model flag or set in the configuration. Available: ${SupportedModel.options.join(', ')}`,
    );
  }
  return model;
}

/**
 * Parses and consolidates configuration from all sources.
 */
export async function resolveAppConfig(
  argv: GenerateCommandArgs,
): Promise<Config> {
  const userConfig = await readConfigFile();
  const model = resolveModel(argv.model, userConfig);

  return parseConfig({
    model,
    batchSize: argv['batch-size'],
    maxTokens: argv['max-tokens'],
    temperature: argv.temperature,
    retries: argv.retries,
    dryRun: argv['dry-run'],
    requireResultTag: false, // Not used for generation
  });
}

/**
 * Extracts the generation-specific configuration.
 */
export function getGenerationConfig(appConfig: Config): GenerationConfig {
  return {
    apiKey: appConfig.apiKey,
    apiBaseUrl: appConfig.apiBaseUrl,
    model: appConfig.model,
    temperature: appConfig.temperature,
    maxTokens: appConfig.maxTokens,
    retries: appConfig.retries,
    batchSize: appConfig.batchSize,
  };
}

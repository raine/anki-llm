import { promises as fs } from 'fs';
import path from 'path';
import os from 'os';

const configDir = path.join(os.homedir(), '.config', 'anki-llm-batch');
const configPath = path.join(configDir, 'config.json');

export interface PersistentConfig {
  model?: string;
  [key: string]: unknown;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value);
}

function isNodeError(error: unknown): error is NodeJS.ErrnoException {
  return (
    typeof error === 'object' &&
    error !== null &&
    'code' in error &&
    typeof (error as { code?: unknown }).code === 'string'
  );
}

export async function readConfigFile(): Promise<PersistentConfig> {
  try {
    await fs.mkdir(configDir, { recursive: true });
    const content = await fs.readFile(configPath, 'utf-8');
    const parsed = JSON.parse(content) as unknown;

    if (!isRecord(parsed)) {
      throw new Error('Config file must contain a JSON object.');
    }

    const { model, ...rest } = parsed;

    if (model !== undefined && typeof model !== 'string') {
      throw new Error('Config "model" must be a string when present.');
    }

    if (typeof model === 'string') {
      return {
        ...(rest as Record<string, unknown>),
        model,
      } as PersistentConfig;
    }

    return rest as PersistentConfig;
  } catch (error: unknown) {
    if (isNodeError(error) && error.code === 'ENOENT') {
      return {};
    }
    throw error;
  }
}

export async function writeConfigFile(config: PersistentConfig): Promise<void> {
  await fs.mkdir(configDir, { recursive: true });
  await fs.writeFile(configPath, JSON.stringify(config, null, 2), 'utf-8');
}

export function getConfigPath(): string {
  return configPath;
}

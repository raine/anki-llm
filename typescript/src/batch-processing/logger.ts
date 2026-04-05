import { writeFile, appendFile } from 'fs/promises';
import chalk from 'chalk';
import { stripAnsi } from './util.js';

// Module-scoped log file path
let logFilePath: string | null = null;

// Module-scoped verbose flag
let isVerbose = false;

/**
 * Initializes the logger with a log file path and clears any previous log file.
 */
export async function initLogger(
  path: string,
  verbose: boolean = false,
): Promise<void> {
  logFilePath = path;
  isVerbose = verbose;
  await writeFile(logFilePath, '', 'utf-8');
}

/**
 * Logs a message to the log file only (for debugging/audit).
 * ANSI color codes are automatically stripped.
 */
export async function logDebug(message: string): Promise<void> {
  if (!logFilePath) return;

  const cleanMessage = stripAnsi(message);
  const timestamp = new Date().toISOString();
  const logEntry = `[${timestamp}] ${cleanMessage}\n`;

  try {
    await appendFile(logFilePath, logEntry, 'utf-8');
  } catch (error) {
    // Don't let log failures crash the program
    const errorMsg = error instanceof Error ? error.message : String(error);
    console.error(chalk.red(`Failed to write to log file: ${errorMsg}`));
  }
}

/**
 * Logs a message to the console only (user-facing).
 * Can include chalk formatting.
 */
export function logInfo(message: string): void {
  console.log(message);
}

/**
 * Logs a message to both the console and the log file.
 * Can include chalk formatting for the console (will be stripped in the file).
 * Handles leading/trailing newlines properly - they appear in console but not in log timestamps.
 */
export async function logInfoTee(message: string): Promise<void> {
  // Print to console with original formatting (including newlines)
  logInfo(message);

  // For log file, trim the message so newlines don't create blank timestamped lines
  const trimmedMessage = message.trim();
  if (trimmedMessage) {
    await logDebug(trimmedMessage);
  }
}

/**
 * Logs an error message to both console and log file.
 */
export async function logError(
  message: string,
  error?: unknown,
): Promise<void> {
  const errorMessage = error instanceof Error ? error.message : String(error);
  console.error(chalk.red(`\n❌ Error: ${message}`));
  if (error) {
    console.error(error);
  }
  await logDebug(`ERROR: ${message}. Details: ${errorMessage}`);
}

/**
 * Logs verbose information (like LLM responses) to the log file only.
 * Only logs if verbose mode is enabled.
 */
export async function logVerbose(message: string): Promise<void> {
  if (!isVerbose) return;
  await logDebug(`[VERBOSE] ${message}`);
}

// Legacy compatibility - marked as deprecated
/** @deprecated Use logDebug() instead */
export async function log(message: string): Promise<void> {
  await logDebug(message);
}

import { writeFile, appendFile } from 'fs/promises';
import chalk from 'chalk';
import { stripAnsi } from './util.js';

// Module-scoped log file path
let logFilePath: string | null = null;

/**
 * Initializes the logger with a log file path and clears any previous log file.
 */
export async function initLogger(path: string): Promise<void> {
  logFilePath = path;
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
 */
export async function logInfoTee(message: string): Promise<void> {
  logInfo(message);
  await logDebug(message);
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

// Legacy compatibility - marked as deprecated
/** @deprecated Use logDebug() instead */
export async function log(message: string): Promise<void> {
  await logDebug(message);
}

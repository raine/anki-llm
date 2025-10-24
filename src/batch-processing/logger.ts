import { writeFile, appendFile } from 'fs/promises';
import chalk from 'chalk';

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
 * Logs a message to both console and log file
 */
export async function log(message: string, skipConsole = false): Promise<void> {
  const timestamp = new Date().toISOString();
  const logEntry = `[${timestamp}] ${message}\n`;

  if (!skipConsole) {
    console.log(message);
  }

  if (logFilePath) {
    try {
      await appendFile(logFilePath, logEntry, 'utf-8');
    } catch (error) {
      // Don't let log failures crash the program
      const errorMsg = error instanceof Error ? error.message : String(error);
      console.error(chalk.red(`Failed to write to log file: ${errorMsg}`));
    }
  }
}

import { writeFile, readFile } from 'fs/promises';
import { existsSync } from 'fs';
import * as path from 'path';
import Papa from 'papaparse';
import * as yaml from 'js-yaml';
import chalk from 'chalk';
import type { ValidatedCard } from '../types.js';

type CardData = Record<string, string>;

/**
 * Checks if the headers of new data match existing headers.
 * @param existingHeaders - Array of headers from the existing file.
 * @param newHeaders - Array of headers from the new data.
 * @returns true if headers match (ignoring order).
 */
function headersMatch(
  existingHeaders: string[],
  newHeaders: string[],
): boolean {
  if (existingHeaders.length !== newHeaders.length) {
    return false;
  }
  const existingSet = new Set(existingHeaders);
  for (const header of newHeaders) {
    if (!existingSet.has(header)) {
      return false;
    }
  }
  return true;
}

/**
 * Exports selected cards to a file (YAML or CSV) in a format compatible with
 * the anki-llm import command. If the file exists, appends to it after
 * validating schema compatibility.
 *
 * The output is a simple array of objects where keys are Anki field names,
 * matching the format expected by the import command.
 *
 * @param cards - The validated and selected cards to export
 * @param outputPath - The path to the output file
 */
export async function exportCards(
  cards: ValidatedCard[],
  outputPath: string,
): Promise<void> {
  const dataToExport: CardData[] = cards.map((card) => card.ankiFields);
  const ext = path.extname(outputPath).toLowerCase();
  const newCardHeaders =
    dataToExport.length > 0 ? Object.keys(dataToExport[0]) : [];

  let fileContent: string;
  let isAppending = false;

  // Check if file exists and append to it
  if (existsSync(outputPath)) {
    isAppending = true;
    console.log(
      chalk.cyan(
        `\n📦 Appending ${cards.length} card(s) to existing ${outputPath}...`,
      ),
    );

    const existingContent = await readFile(outputPath, 'utf-8');
    let existingData: CardData[] = [];

    // Read and validate existing file
    try {
      if (ext === '.yaml' || ext === '.yml') {
        const parsed = yaml.load(existingContent);
        if (Array.isArray(parsed)) {
          existingData = parsed as CardData[];
        } else if (parsed) {
          throw new Error('Existing YAML is not an array.');
        }
      } else if (ext === '.csv') {
        if (existingContent.trim()) {
          const parsed = Papa.parse<CardData>(existingContent, {
            header: true,
            skipEmptyLines: true,
          });
          if (parsed.errors.length > 0) {
            throw new Error(
              `Failed to parse existing CSV: ${parsed.errors[0].message}`,
            );
          }
          existingData = parsed.data;
        }
      } else {
        throw new Error(
          `Unsupported output format: ${ext}. Please use .csv, .yaml, or .yml.`,
        );
      }
    } catch (e) {
      throw new Error(
        `Failed to parse existing file at ${outputPath}. It may be corrupted or in the wrong format. Error: ${(e as Error).message}`,
      );
    }

    // Schema validation
    if (existingData.length > 0) {
      const existingHeaders = Object.keys(existingData[0]);
      if (!headersMatch(existingHeaders, newCardHeaders)) {
        throw new Error(
          `Schema mismatch: Cannot append cards.\n` +
            `Existing file fields: ${existingHeaders.join(', ')}\n` +
            `New card fields:      ${newCardHeaders.join(', ')}`,
        );
      }
    }

    // Combine data
    const combinedData = [...existingData, ...dataToExport];
    if (ext === '.yaml' || ext === '.yml') {
      fileContent = yaml.dump(combinedData);
    } else {
      // .csv
      fileContent = Papa.unparse(combinedData);
    }

    console.log(
      chalk.gray(
        `  Found ${existingData.length} existing card(s), adding ${cards.length} new card(s)`,
      ),
    );
  } else {
    // File doesn't exist, create new
    console.log(
      chalk.cyan(`\n📦 Exporting ${cards.length} card(s) to ${outputPath}...`),
    );

    if (ext === '.yaml' || ext === '.yml') {
      fileContent = yaml.dump(dataToExport);
    } else if (ext === '.csv') {
      fileContent = Papa.unparse(dataToExport);
    } else {
      throw new Error(
        `Unsupported output format: ${ext}. Please use .csv, .yaml, or .yml.`,
      );
    }
  }

  await writeFile(outputPath, fileContent, 'utf-8');

  console.log(
    chalk.green(
      `\n✓ Successfully ${isAppending ? 'appended to' : 'exported cards to'} ${outputPath}`,
    ),
  );
  console.log(chalk.gray('\nTo import this file into Anki, run:'));
  console.log(
    chalk.white(`  anki-llm import "${outputPath}" --deck "Your Deck Name"`),
  );
}

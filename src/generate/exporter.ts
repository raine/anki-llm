import { writeFile } from 'fs/promises';
import * as path from 'path';
import Papa from 'papaparse';
import * as yaml from 'js-yaml';
import chalk from 'chalk';
import type { ValidatedCard } from './validator.js';

/**
 * Exports selected cards to a file (YAML or CSV) in a format compatible with
 * the anki-llm import command.
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
  console.log(
    chalk.cyan(`\n📦 Exporting ${cards.length} card(s) to ${outputPath}...`),
  );

  // Extract just the ankiFields for export (compatible with import command)
  const dataToExport = cards.map((card) => card.ankiFields);
  const ext = path.extname(outputPath).toLowerCase();

  let fileContent: string;

  if (ext === '.yaml' || ext === '.yml') {
    fileContent = yaml.dump(dataToExport);
  } else if (ext === '.csv') {
    fileContent = Papa.unparse(dataToExport);
  } else {
    throw new Error(
      `Unsupported output format: ${ext}. Please use .csv, .yaml, or .yml.`,
    );
  }

  await writeFile(outputPath, fileContent, 'utf-8');

  console.log(chalk.green(`\n✓ Successfully exported cards to ${outputPath}`));
  console.log(chalk.gray('\nTo import this file into Anki, run:'));
  console.log(
    chalk.white(`  anki-llm import "${outputPath}" --deck "Your Deck Name"`),
  );
}

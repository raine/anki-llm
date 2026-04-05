import { readFile, writeFile, rename, mkdtemp } from 'fs/promises';
import Papa from 'papaparse';
import * as path from 'path';
import * as yaml from 'js-yaml';
import { tmpdir } from 'os';
import type { RowData, ProcessedRow } from './types.js';
import { requireNoteId } from './util.js';

/**
 * Atomically writes a file by writing to a temp file first, then renaming.
 * This prevents partial/corrupted files if the process crashes mid-write.
 */
export async function atomicWriteFile(
  filePath: string,
  data: string,
): Promise<void> {
  const dir = await mkdtemp(path.join(tmpdir(), 'batch-ai-'));
  const tmpPath = path.join(dir, path.basename(filePath));
  await writeFile(tmpPath, data, 'utf-8');
  await rename(tmpPath, filePath); // atomic on POSIX systems
}

/**
 * Parses a data file (CSV or YAML) into an array of row objects.
 */
export async function parseDataFile(filePath: string): Promise<RowData[]> {
  const fileContent = await readFile(filePath, 'utf-8');
  const extension = path.extname(filePath).toLowerCase();

  if (extension === '.csv') {
    const parseResult = Papa.parse<RowData>(fileContent, {
      header: true,
      skipEmptyLines: true,
    });
    if (parseResult.errors.length > 0) {
      throw new Error(
        `CSV parsing errors: ${JSON.stringify(parseResult.errors)}`,
      );
    }
    return parseResult.data;
  } else if (extension === '.yml' || extension === '.yaml') {
    const data = yaml.load(fileContent);
    // Ensure the YAML content is an array of objects
    if (!Array.isArray(data)) {
      throw new Error('YAML content is not an array');
    }
    // eslint-disable-next-line @typescript-eslint/no-unsafe-return
    return data;
  } else {
    throw new Error(
      `Unsupported file extension: ${extension}. Use .csv, .yaml, or .yml`,
    );
  }
}

/**
 * Serializes an array of row objects to a string (CSV or YAML).
 */
export function serializeData(rows: ProcessedRow[], filePath: string): string {
  const extension = path.extname(filePath).toLowerCase();

  if (extension === '.csv') {
    return Papa.unparse(rows, {
      quotes: true,
      newline: '\n',
      header: true,
    });
  } else if (extension === '.yml' || extension === '.yaml') {
    return yaml.dump(rows, { lineWidth: -1 });
  } else {
    throw new Error(
      `Unsupported file extension: ${extension}. Use .csv, .yaml, or .yml`,
    );
  }
}

/**
 * Loads existing output file and creates a map of rows by noteId
 * Returns empty map if file doesn't exist
 */
export async function loadExistingOutput(
  filePath: string,
): Promise<Map<string, RowData>> {
  try {
    const rows = await parseDataFile(filePath);
    const map = new Map<string, RowData>();
    for (const row of rows) {
      const noteId = requireNoteId(row);
      map.set(noteId, row);
    }
    return map;
  } catch {
    // File doesn't exist or can't be parsed - return empty map
    return new Map();
  }
}

import { writeFile } from "fs/promises";

const ANKI_CONNECT_URL = "http://127.0.0.1:8765";
const DECK_NAME = "Glossika-ENJA [2001-3000]";
const OUTPUT_FILE = "glossika_deck_export.csv";

interface AnkiConnectPayload {
  action: string;
  params: Record<string, unknown>;
  version: number;
}

interface AnkiConnectResponse<T = unknown> {
  result: T | null;
  error: string | null;
}

interface NoteField {
  value: string;
  order: number;
}

interface NoteInfo {
  noteId: number;
  fields: Record<string, NoteField>;
  tags: string[];
  modelName: string;
}

interface CsvRow {
  id: string;
  english: string;
  japanese: string;
  ka: string;
  ROM: string;
  explanation: string;
}

/**
 * Helper function to send requests to AnkiConnect.
 */
async function ankiRequest<T = unknown>(
  action: string,
  params: Record<string, unknown> = {}
): Promise<T> {
  const payload: AnkiConnectPayload = { action, params, version: 6 };

  try {
    const response = await fetch(ANKI_CONNECT_URL, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
      },
      body: JSON.stringify(payload),
    });

    if (!response.ok) {
      throw new Error(`HTTP error! status: ${response.status}`);
    }

    const responseJson = (await response.json()) as AnkiConnectResponse<T>;

    if (responseJson.error) {
      throw new Error(`AnkiConnect API error: ${responseJson.error}`);
    }

    return responseJson.result as T;
  } catch (error) {
    if (error instanceof Error) {
      throw new Error(
        `Could not connect to AnkiConnect. Is Anki running? Error: ${error.message}`
      );
    }
    throw error;
  }
}

/**
 * Converts an array of rows to CSV format.
 */
function rowsToCsv(rows: CsvRow[]): string {
  const fieldNames: (keyof CsvRow)[] = [
    "id",
    "english",
    "japanese",
    "ka",
    "ROM",
    "explanation",
  ];

  // CSV header
  const header = fieldNames.join(",");

  // CSV rows
  const csvRows = rows.map((row) => {
    return fieldNames
      .map((field) => {
        const value = row[field];
        // Escape quotes and wrap in quotes if the value contains comma, quote, or newline
        if (value.includes(",") || value.includes('"') || value.includes("\n")) {
          return `"${value.replace(/"/g, '""')}"`;
        }
        return value;
      })
      .join(",");
  });

  return [header, ...csvRows].join("\n");
}

async function exportDeckToCsv(): Promise<void> {
  console.log("=".repeat(60));
  console.log(`Exporting deck: ${DECK_NAME}`);
  console.log("=".repeat(60));

  try {
    // Find all notes in the deck
    const query = `deck:"${DECK_NAME}"`;
    const noteIds = await ankiRequest<number[]>("findNotes", { query });
    console.log(`\n✓ Found ${noteIds.length} notes in '${DECK_NAME}'.`);

    if (noteIds.length === 0) {
      console.log("No notes found to export.");
      return;
    }

    // Get detailed info for all notes
    console.log(`\nFetching note details...`);
    const notesInfo = await ankiRequest<NoteInfo[]>("notesInfo", {
      notes: noteIds,
    });
    console.log(`✓ Retrieved information for ${notesInfo.length} notes.`);

    // Convert notes to CSV rows
    const rows: CsvRow[] = notesInfo.map((note) => {
      const fields = note.fields;
      return {
        id: fields.Id?.value?.replace(/\r/g, "") || "",
        english: fields.English?.value?.replace(/\r/g, "") || "",
        japanese: fields.Japanese?.value?.replace(/\r/g, "") || "",
        ka: fields["か"]?.value?.replace(/\r/g, "") || "",
        ROM: fields.ROM?.value?.replace(/\r/g, "") || "",
        explanation: fields.Explanation?.value?.replace(/\r/g, "") || "",
      };
    });

    // Write to CSV
    console.log(`\nWriting to ${OUTPUT_FILE}...`);
    const csvContent = rowsToCsv(rows);

    await writeFile(OUTPUT_FILE, csvContent, "utf-8");
    console.log(
      `✓ Successfully exported ${notesInfo.length} notes to ${OUTPUT_FILE}`
    );
  } catch (error) {
    console.log(`\n✗ Error: ${error}`);
    console.log("\nMake sure:");
    console.log("  1. Anki Desktop is running");
    console.log("  2. AnkiConnect add-on is installed (code: 2055492159)");
    console.log(`  3. Deck '${DECK_NAME}' exists`);
  }
}

exportDeckToCsv();

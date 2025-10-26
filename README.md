# anki-llm-batch

A command-line interface for bulk-processing Anki flashcards with LLMs.

## Why?

Manually editing hundreds or thousands of Anki cards is tedious, error-prone,
and time-consuming. Whether you're fixing translations, generating example
sentences, or adding phonetic readings, doing it one-by-one is a non-starter.

`anki-llm-batch` provides a bridge between your Anki collection and modern AI
models. It allows you to **export** your data, **batch process** it against a
custom prompt, and then **import** the results back to Anki.

The general workflow is a three-step process:

1.  Export: Pull notes from a specified Anki deck into a CSV or YAML.
2.  Process: Run an AI model (e.g., GPT-4o mini) on a specific field in every
    note, using a prompt template.
3.  Import: Update the original notes by importing the CSV or YAML back to Anki.

## Features

- **Export** Anki decks to clean CSV or YAML files.
- **Batch process** note fields using any OpenAI-compatible API.
- **Custom prompts**: Use flexible template files to define exactly how the AI
  should process your data.
- **Concurrent processing**: Make multiple parallel API requests to speed up
  large jobs.
- **Resilient**: Automatically retries failed requests and saves progress
  incrementally.
- **Smart updates**: Imports data back into Anki by updating existing notes, not
  creating duplicates.

## Installation

Install globally via npm:

```bash
npm install -g anki-llm-batch
```

## Requirements

- Node.js (v18 or higher)
- Anki Desktop must be running.
- The [AnkiConnect](https://ankiweb.net/shared/info/2055492159) add-on must be
  installed in Anki.

## API Configuration

`anki-llm-batch` uses LLM APIs to process your notes. You need to configure an
API key for the model provider you want to use.

### Supported Models

The tool supports two API providers:

| Model                    | Input   | Output   |                                                                          |
| ------------------------ | ------- | -------- | ------------------------------------------------------------------------ |
| **OpenAI models**        |
| `gpt-4.1`                | $2.50/M | $10.00/M | [🔗](https://platform.openai.com/docs/models/gpt-4.1)                    |
| `gpt-4o`                 | $2.50/M | $10.00/M | [🔗](https://platform.openai.com/docs/models/gpt-4o)                     |
| `gpt-4o-mini` (default)  | $0.15/M | $0.60/M  | [🔗](https://platform.openai.com/docs/models/gpt-4o-mini)                |
| `gpt-5-nano`             | $0.05/M | $0.40/M  | [🔗](https://platform.openai.com/docs/models/gpt-5-nano)                 |
| **Google Gemini models** |
| `gemini-2.0-flash`       | $0.10/M | $0.40/M  | [🔗](https://ai.google.dev/gemini-api/docs/models#gemini-2.0-flash)      |
| `gemini-2.5-flash`       | $0.30/M | $2.50/M  | [🔗](https://ai.google.dev/gemini-api/docs/models#gemini-2.5-flash)      |
| `gemini-2.5-flash-lite`  | $0.10/M | $0.40/M  | [🔗](https://ai.google.dev/gemini-api/docs/models#gemini-2.5-flash-lite) |

Pricing is per million tokens (M).

### Setting up API Keys

Set the appropriate environment variable for your chosen model provider:

**For OpenAI models:**

```bash
export OPENAI_API_KEY="your-api-key-here"
```

Get your API key from: https://platform.openai.com/api-keys

**For Gemini models:**

```bash
export GEMINI_API_KEY="your-api-key-here"
```

Get your API key from: https://aistudio.google.com/api-keys

## Commands reference

### `anki-llm-batch export <deck> [output]`

Exports notes from an Anki deck.

- `<deck>`: The name of the Anki deck to export (must be in quotes if it
  contains spaces).
- `[output]`: Optional output file path. If omitted, automatically generates a
  filename from the deck name (e.g., `"My Deck"` → `my-deck.yaml`). You can also
  provide just a file extension (e.g., `.csv`) to auto-generate the filename
  with your preferred format.

**Examples:**

```bash
# Auto-generate filename with default .yaml format
anki-llm-batch export "Japanese Core 1k"
# → japanese-core-1k.yaml

# Auto-generate filename with .csv format
anki-llm-batch export "Japanese Core 1k" .csv
# → japanese-core-1k.csv

# Specify custom filename
anki-llm-batch export "Japanese Core 1k" my-custom-name.yaml
```

### `anki-llm-batch process <input>`

Processes a data file using an AI model.

- `<input>`: Path to the input data file (CSV or YAML).

**Required options:**

- `-o, --output`: Path to write the processed data file.
- `-p, --prompt`: Path to the prompt template text file.
- **Either** `--field` **or** `--json` (mutually exclusive):
  - `--field <name>`: Update a single field with the AI response.
  - `--json`: Expect JSON response and merge all fields into the note.

**Common options:**

- `-m, --model`: OpenAI model to use (default: `gpt-4o-mini`).
- `-b, --batch-size`: Number of concurrent API requests (default: `5`).
- `-r, --retries`: Number of retries for failed requests (default: `3`).
- `-d, --dry-run`: Preview the operation without making API calls.
- `-f, --force`: Re-process all notes, even if they exist in the output file.
- `--limit`: Limit the number of new rows to process (useful for testing prompts
  on a small sample before processing large datasets).
- `--require-result-tag`: Only extracts content from within `<result></result>`
  tags in the AI response.
- `--log`: Generate a log file with detailed debug information.
- `--very-verbose`: Log full LLM responses to the log file (automatically
  enables `--log`). Useful for debugging prompts and understanding model
  outputs.

**Understanding `--field` vs `--json` modes:**

- **`--field` mode** (single field update): The AI response is saved to the
  specified field. This is the traditional mode for updating one field at a
  time.

  ```bash
  anki-llm-batch process notes.yaml -o out.yaml --field Translation -p prompt.txt
  ```

- **`--json` mode** (multi-field merge): The AI must return valid JSON. All
  fields in the JSON are merged into your note, allowing multi-field updates.

  ```bash
  anki-llm-batch process notes.yaml -o out.yaml --json -p prompt.txt
  ```

  Example: If your note has `Japanese` and `Grammar` fields, and the AI returns:

  ```json
  {
    "Japanese": "こんにちは",
    "Grammar": "greeting"
  }
  ```

  Both fields will be updated. Only fields present in the JSON are updated
  (partial updates are allowed). If the response is not valid JSON, the
  operation will fail and retry.

### `anki-llm-batch import <input>`

Imports data from a file into an Anki deck, updating existing notes.

- `<input>`: Path to the data file to import (CSV or YAML).

**Required options:**

- `-d, --deck`: The name of the target Anki deck.

**Common options:**

- `-m, --model`: The name of the Anki note type/model to use. If not specified,
  the model will be inferred from existing notes in the deck.
- `-k, --key-field`: Field to use for identifying existing notes (default:
  `noteId`).

### `anki-llm-batch query <action> [params]`

Query the AnkiConnect API directly with any supported action. This command is
especially useful for AI agents (like Claude Code) to explore and interact with
your Anki collection programmatically.

- `<action>`: The AnkiConnect API action to perform (e.g., `deckNames`,
  `findNotes`, `cardsInfo`).
- `[params]`: Optional JSON string of parameters for the action.

**Why this is useful for AI agents:**

AI assistants can use this command to dynamically query your Anki collection
without you having to manually provide information. For example:

- "List all my decks" → `anki-llm-batch query deckNames`
- "Show me statistics for my Japanese deck" →
  `anki-llm-batch query getDeckStats '{"decks":["Japanese"]}'`
- "Find all cards with tag 'vocabulary'" →
  `anki-llm-batch query findNotes '{"query":"tag:vocabulary"}'`

The command outputs clean JSON that AI agents can parse and reason about, making
it easy to build custom workflows or answer questions about your Anki
collection.

**Examples:**

```bash
# Get all deck names
anki-llm-batch query deckNames

# Get all model (note type) names
anki-llm-batch query modelNames

# Find notes in a specific deck
anki-llm-batch query findNotes '{"query":"deck:Japanese"}'

# Get detailed information about specific cards
anki-llm-batch query cardsInfo '{"cards":[1498938915662]}'

# Get statistics for a deck
anki-llm-batch query getDeckStats '{"decks":["Default"]}'

# Check AnkiConnect version
anki-llm-batch query version

# Get full AnkiConnect API documentation (useful for AI agents to understand available actions)
anki-llm-batch query docs
```

**Real-world AI agent example:**

Here's a real example of Claude Code autonomously exporting all Anki decks when
given the simple instruction: "Use anki-llm-batch to export my decks in csv"

```
> Use anki-llm-batch to export my decks in csv

⏺ I'll help you export your Anki decks to CSV. Let me first check
  the available commands:

⏺ Bash(anki-llm-batch --help)
  ⎿  anki-llm-batch <command>

     Commands:
       anki-llm-batch export <deck> <output>              Export deck to file
       anki-llm-batch process <input> <output> <field>    Process notes with AI
       anki-llm-batch import <input> <deck> <model>       Import data to deck
       anki-llm-batch query <action> [params]             Query AnkiConnect API

⏺ Now let me check what decks are available:

⏺ Bash(anki-llm-batch query deckNames)
  ⎿  [
       "_",
       "_::Genki I",
       "_::Genki II",
       "_::Japanese Counters",
       "Glossika-ENJA [0001-1000]",
       "Japanese Speaking Practice",
       ... 7 more decks
     ]

⏺ I can see you have several Anki decks. I'll export each one to CSV.

⏺ Bash(anki-llm-batch export "_::Genki I" "genki_i.csv")
  ⎿  ✓ Found 342 notes in '_::Genki I'.
     ✓ Successfully exported 342 notes to genki_i.csv

⏺ Bash(anki-llm-batch export "_::Genki II" "genki_ii.csv")
  ⎿  ✓ Found 315 notes in '_::Genki II'.
     ✓ Successfully exported 315 notes to genki_ii.csv

     ... exports continue for remaining 9 decks ...

⏺ Bash(ls -lh *.csv)
     -rw-r--r--  170K  genki_i.csv
     -rw-r--r--  138K  genki_ii.csv
     -rw-r--r--   95K  japanese_counters.csv
     -rw-r--r--  394K  glossika_enja_0001_1000.csv
     ... 7 more files

⏺ Successfully exported all your Anki decks to CSV!

  Summary: 12 files, ~4.6MB total
  - genki_i.csv (170K) - 342 notes
  - genki_ii.csv (138K) - 315 notes
  - glossika_enja_0001_1000.csv (394K) - 1,000 notes
  ... and 8 more decks
```

The AI agent used `query deckNames` to discover available decks, then
automatically generated appropriate filenames and exported all 12 decks without
further human intervention. This demonstrates how the `query` command enables AI
agents to work autonomously with your Anki collection.

**Another example: Sampling random cards from decks**

Here's another example of Claude Code using the `query` command to sample random
cards from multiple decks. Given the instruction: "Use anki-llm-batch to pick
random cards from Glossika decks, and print the English and Japanese fields for
each, pick 10 cards from each deck, and save to a markdown file"

[Full conversation](https://gist.github.com/raine/b8d42275a188005bd2dadc34b8e05824)

This demonstrates how the `query` command enables AI agents to build custom
scripts for data analysis and extraction tasks autonomously.

**Special actions:**

- `docs` or `help`: Returns the complete AnkiConnect API documentation. This is
  especially useful for AI agents that need to understand what actions are
  available and how to use them. The agent can query this once to get the full
  documentation and then use that context to make informed decisions about which
  API calls to make.

See [ANKI_CONNECT.md](./ANKI_CONNECT.md) for the complete list of available
actions and their parameters.

## Example use case: Fixing 1000 japanese translations

Let's say you have an Anki deck named "Japanese Core 1k" with 1000 notes. Each
note has a `Japanese` field with a sentence and a `Translation` field with an
English translation that you suspect is inaccurate. We'll use `anki-llm-batch`
and GPT-4o mini to generate better translations for all 1000 notes.

### Step 1: Export your deck

First, export the notes from your Anki deck into a YAML file. YAML is great for
multiline text fields and for using `git diff` to see what has changed after
processing is complete.

```bash
anki-llm-batch export "Japanese Core 1k" notes.yaml
```

This command will connect to Anki, find all notes in that deck, and save them to
a YAML file.

```
============================================================
Exporting deck: Japanese Core 1k
============================================================

✓ Found 1000 notes in 'Japanese Core 1k'.

Discovering model type and fields...
✓ Model type: Japanese Model
✓ Fields: Japanese, Translation, Reading, Sound, noteId

Fetching note details...
✓ Retrieved information for 1000 notes.

Writing to notes.yaml...
✓ Successfully exported 1000 notes to notes.yaml
```

The `notes.yaml` file will look something like this:

```yaml
- noteId: 1512345678901
  Japanese: 猫は机の上にいます。
  Translation: The cat is on the desk.
- noteId: 1512345678902
  Japanese: 彼は毎日公園を散歩します。
  Translation: He strolls in the park every day.
# ... 998 more notes
```

### Step 2: Create a prompt template

Next, create a prompt file (`prompt-ja-en.txt`) to instruct the AI. Use
`{field_name}` syntax for variables that will be replaced with data from each
note. We want to process the `Japanese` field.

**File: `prompt-ja-en.txt`**

```
You are an expert Japanese-to-English translator.

Translate this Japanese sentence to English: {Japanese}

Guidelines:
- Translate accurately while preserving nuance and meaning.
- Be natural and idiomatic in English.
- If possible, structure the English so the original Japanese grammar can be inferred.

Instructions:
1. First, analyze the sentence structure and key elements.
2. Think through the translation choices and any nuances.
3. Provide your final translation wrapped in <result></result> XML tags.

Format your response like this:
- Analysis: [your analysis of the sentence]
- Translation considerations: [your thought process]
- <result>[your final English translation here]</result>
```

<!-- prettier-ignore -->
> [!NOTE]
> The `<result>` tag (used with `--require-result-tag`) is optional. You could instruct the LLM to respond with only the translation directly. However, asking the model to "think out loud" by analyzing the sentence first tends to produce higher-quality translations, as it encourages deeper reasoning before generating the final output.

### Step 3: Run the process command

Now, run the `process` command. We'll tell it to use our `notes.yaml` file as
input, write to a new `notes-translated.yaml` file, process the `Translation`
field, and use our prompt template.

The tool will read the `Japanese` field from each note to fill the prompt, then
the AI's response will overwrite the `Translation` field.

```bash
anki-llm-batch process notes.yaml \
  --output notes-translated.yaml \
  --field Translation \
  --prompt prompt-ja-en.txt \
  --batch-size 10 \
  --require-result-tag
```

- `notes.yaml`: The input file.
- `--output notes-translated.yaml`: The output file.
- `--field Translation`: The field we want the AI to generate and place its
  result into.
- `--prompt prompt-ja-en.txt`: Our instruction template.
- `--batch-size 10`: Process 10 notes concurrently for speed.
- `--require-result-tag`: Ensures the tool only saves the content inside the
  `<result>` tag, ignoring the AI's analysis.

You will see real-time progress as it processes the notes:

```
============================================================
Batch AI Data Processing
============================================================
Input file:        notes.yaml
Output file:       notes-translated.yaml
Field to process:  Translation
Model:             gpt-4o-mini
Batch size:        10
...
============================================================

Reading notes.yaml...
✓ Found 1000 rows in YAML

Loading existing output...
✓ Found 0 already-processed rows

Processing 1000 rows...
Processing |████████████████████████████████████████| 100% | 1000/1000 rows | Cost: $0.0234 | Tokens: 152340

✓ Processing complete

============================================================
Summary
============================================================
- Successes:         1000
- Failures:          0
- Total Processed:   1000
- Total Time:        85.32s
- Model:             gpt-4o-mini
- Dry Run:           false
---
- Total Tokens:      152,340
- Input Tokens:      120,100
- Output Tokens:     32,240
- Est. Cost:         $0.02
============================================================
```

### Step 4: Import the changes

The final step is to import the newly generated translations back into Anki. The
tool uses the `noteId` to find and update the existing notes.

```bash
anki-llm-batch import notes-translated.yaml --deck "Japanese Core 1k"
```

- `notes-translated.yaml`: The file with our improved translations.
- `--deck "Japanese Core 1k"`: The destination deck.

The model type will be automatically inferred from the existing notes in the
deck. You can also explicitly specify it with `--model "Japanese Model"` if
needed.

```
============================================================
Importing from notes-translated.yaml to deck: Japanese Core 1k
Model: Japanese Model
Key field: noteId
============================================================

✓ Found 1000 rows in notes-translated.yaml.

✓ Valid fields to import: Japanese, Translation, Reading, Sound

✓ Found 1000 existing notes with a 'noteId' field.

✓ Partitioning complete:
  - 0 new notes to add.
  - 1000 existing notes to update.

Updating 1000 existing notes...
✓ Update operation complete: 1000 notes updated successfully.

Import process finished.
```

That's it! All 1000 notes in your Anki deck have now been updated with
high-quality translations.

## Development

### Running locally

Use `tsx` to run the CLI directly from TypeScript source without rebuilding:

```bash
pnpm tsx src/cli.ts export "My Deck" notes.yaml
```

### Testing the global CLI

Use `pnpm link` to test the command globally:

```bash
pnpm link --global
anki-llm-batch export "My Deck" notes.yaml
```

Note: The linked command uses compiled JavaScript from `dist/`. Run
`pnpm run build` after making changes to see them reflected.

To unlink: `pnpm unlink --global`

### Code quality

```bash
pnpm run check
```

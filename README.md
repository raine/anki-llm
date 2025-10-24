# anki-llm-batch

A command-line interface for bulk-processing Anki flashcards with LLMs.

## why?

Manually editing hundreds or thousands of Anki cards is tedious, error-prone,
and time-consuming. Whether you're fixing translations, generating example
sentences, or adding phonetic readings, doing it one-by-one is a non-starter.

`anki-llm-batch` provides a bridge between your Anki collection and modern AI
models. It allows you to **export** your data, **batch process** it against a
custom prompt, and then **import** the results back to Anki.

The general workflow is a three-step process:

1.  Export: Pull notes from a specified Anki deck into a CSV or YAML.
2.  Batch: Run an AI model (e.g., GPT-4o mini) on a specific field in every
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

## Commands reference

### `anki-llm-batch export <deck> <output>`

Exports notes from an Anki deck.

- `<deck>`: The name of the Anki deck to export (must be in quotes if it
  contains spaces).
- `<output>`: The path for the output file (e.g., `output.csv` or `data.yaml`).

### `anki-llm-batch batch <input> <output> <field> <prompt>`

Processes a data file using an AI model.

- `<input>`: Path to the input data file (CSV or YAML).
- `<output>`: Path to write the processed data file.
- `<field>`: The name of the field to populate with the AI's result.
- `<prompt>`: Path to the prompt template text file.

**Common options:**

- `-m, --model`: OpenAI model to use (default: `gpt-4o-mini`).
- `-b, --batch-size`: Number of concurrent API requests (default: `5`).
- `-r, --retries`: Number of retries for failed requests (default: `3`).
- `-d, --dry-run`: Preview the operation without making API calls.
- `-f, --force`: Re-process all notes, even if they exist in the output file.
- `--require-result-tag`: Only extracts content from within `<result></result>`
  tags in the AI response.

### `anki-llm-batch import <input> <deck> <model>`

Imports data from a file into an Anki deck, updating existing notes.

- `<input>`: Path to the data file to import (CSV or YAML).
- `<deck>`: The name of the target Anki deck.
- `<model>`: The name of the Anki note type/model to use.

**Common options:**

- `-k, --key-field`: Field to use for identifying existing notes (default:
  `noteId`).

## Example use case: Fixing 1000 japanese translations

Let's say you have an Anki deck named "Japanese Core 1k" with 1000 notes. Each
note has a `Japanese` field with a sentence and a `Translation` field with an
English translation that you suspect is inaccurate. We'll use `anki-llm-batch`
and GPT-4o mini to generate better translations for all 1000 notes.

### Step 1: Export your deck

First, export the notes from your Anki deck into a YAML file. YAML is great for
multiline text fields.

```bash
anki-llm-batch export "Japanese Core 1k" notes.yaml
```

This command will connect to Anki, find all notes in that deck, and save them to
`notes.yaml`.

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

### Step 3: Run the batch process

Now, run the `batch` command. We'll tell it to use our `notes.yaml` file as
input, write to a new `notes-translated.yaml` file, process the `Translation`
field, and use our prompt template.

The tool will read the `Japanese` field from each note to fill the prompt, then
the AI's response will overwrite the `Translation` field.

```bash
anki-llm-batch batch \
  notes.yaml \
  notes-translated.yaml \
  Translation \
  prompt-ja-en.txt \
  --batch-size 10 \
  --require-result-tag
```

- `notes.yaml`: The input file.
- `notes-translated.yaml`: The output file.
- `Translation`: The field we want the AI to generate and place its result into.
- `prompt-ja-en.txt`: Our instruction template.
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
Log file:          notes-translated.log
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
Progress: [████████████████████████████████████████] 1000/1000 | 100% | ETA: 0s

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
anki-llm-batch import notes-translated.yaml "Japanese Core 1k" "Japanese Model"
```

- `notes-translated.yaml`: The file with our improved translations.
- `"Japanese Core 1k"`: The destination deck.
- `"Japanese Model"`: The note type/model name for these notes. You can see this
  when exporting the deck initially.

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

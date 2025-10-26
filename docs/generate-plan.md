# Plan: `anki-llm generate` Feature

## 1. Project Goal

To introduce a new, interactive workflow for generating Anki cards from a single
word or concept. This feature will use LLMs to create contextual examples, which
the user can then selectively add to their collection through a command-line
interface.

## 2. User Story

As an Anki user, I want to create several example cards for a new vocabulary
word. I want a tool to generate these cards, present them for review, and let me
choose which ones to add directly to my Anki deck from the terminal.

---

## 3. Core Feature: `anki-llm generate`

This command takes a term, uses a configured prompt, and initiates an
interactive session to add new cards to Anki.

### 3.1. Command Signature

```bash
anki-llm generate <term> [options]
```

**Arguments:**

- `<term>`: (Required) The word or phrase to generate cards for. Must be quoted
  if it contains spaces.

**Options:**

- `-p, --prompt`: (Required) Path to the prompt template file.
- `-c, --count`: (Optional) The number of card examples to generate. Default:
  `3`.
- `-m, --model`: (Optional) The LLM model to use. Overrides the default set in
  `anki-llm config`.
- `--dry-run`: (Optional) Generate cards and print them to the console as JSON,
  without starting the interactive selection or import process.

### 3.2. The Prompt Template

The prompt file is a text or markdown file containing YAML frontmatter for
configuration and a prompt body with instructions for the LLM.

#### 3.2.1. Frontmatter

The frontmatter is a YAML block enclosed by `---` at the top of the file.

- `deck`: (Required) The target Anki deck name.
- `noteType`: (Required) The name of the Anki note type (model) to be used.
- `map`: (Required) An object mapping the keys from the LLM's JSON output to the
  actual field names in the specified `noteType`. This provides a direct,
  one-to-one translation.

#### 3.2.2. Prompt Body

The prompt body contains the instructions for the LLM. It must instruct the
model to produce a single JSON object where all values are strings, including
any values that require HTML formatting.

- It must include the `{term}` placeholder.
- It must instruct the LLM to return a single, valid JSON object.
- It must include a "one-shot" example demonstrating the exact structure and the
  required HTML formatting for relevant fields.

#### 3.2.3. Example Prompt File

This example instructs the LLM to generate a single string containing an HTML
`<ul>` for the `note` field.

````markdown
---
deck: 'Japanese::Vocabulary'
noteType: 'Japanese (recognition)'
map:
  en: 'English'
  jp: 'Japanese'
  furigana: 'Furigana'
  rom: 'Romaji'
  context: 'Context'
  note: 'Notes'
---

You are an expert assistant who creates one excellent Anki flashcard for a
Japanese vocabulary word. The term to create a card for is: **{term}**

Your output must be a single, valid JSON object. All field values must be
strings. For the `note` field, generate a single string containing a well-formed
HTML unordered list (`<ul>`).

Follow the structure and HTML formatting shown in this example precisely:

```json
{
  "en": "How was your <b>today</b>?",
  "jp": "<b>今日</b>の一日はどうでしたか？",
  "furigana": "<b>今日[きょう]</b> の 一日[いちにち] はどうでしたか？",
  "rom": "Kyou no ichinichi wa dou deshita ka?",
  "context": "Friend to Friend (A common, friendly way to ask about someone's day)",
  "note": "<ul><li>今日(きょう) is the general word for 'today'. Its formal equivalent, 本日(ほんじつ), is used in business or official settings.</li><li>The phrase 一日はどうでしたか is a natural set phrase.</li></ul>"
}
```
````

### 3.3. Execution Flow

1.  **Parse Arguments:** Read `<term>` and all options from the CLI.
2.  **Load & Parse Prompt File:** Read the file specified by `--prompt`. Parse
    the YAML frontmatter and separate it from the prompt body. Validate that
    required frontmatter fields are present.
3.  **Prepare API Calls:** Create an array of `--count` identical prompts, each
    with the `{term}` placeholder replaced.
4.  **Execute in Parallel:** Use the existing batch processing engine to make
    `--count` parallel API calls, configured to use JSON Mode for reliable
    output.
5.  **Collect & Validate Results:** Collect all successful and parsable JSON
    responses into a list of "card candidates". Discard any failed or invalid
    responses.
6.  **Interactive Selection:** If no candidates were generated, exit with an
    error. Otherwise, present the list of candidates to the user in an
    interactive checklist.
7.  **Map and Import:**
    - Once the user confirms their selection, iterate through the chosen cards.
    - For each selected card, use the `map` from the frontmatter to transform
      the JSON object into the final Anki field structure (e.g.,
      `{"English": "...", "Japanese": "..."}`). All values are treated as
      strings and mapped directly.
    - Call the AnkiConnect `addNotes` action with the list of prepared notes.
8.  **Report Outcome:** Print a summary to the user (e.g.,
    `✓ Successfully added 2 new notes to "Japanese::Vocabulary".`).

---

## 4. Onboarding Feature: `anki-llm generate:init`

A one-time scaffolding command will be created to generate a prompt template
based on the user's Anki collection.

### 4.1. Goal

To create a prompt file for the user by interactively guiding them through
configuration, thus lowering the barrier to entry for the `generate` command.

### 4.2. Command Signature

```bash
anki-llm generate:init [output-file]
```

- `[output-file]`: (Optional) The path where the generated prompt file will be
  saved.

### 4.3. Interactive Wizard Flow

1.  **Welcome:** Explain the command's purpose.
2.  **Select Deck:** Call `deckNames` and present the list of decks for user
    selection.
3.  **Confirm Note Type:** Infer the note type from the selected deck and ask
    for user confirmation.
4.  **Map Fields:** Fetch the field names for the note type. Present the user
    with a pre-filled mapping of Anki fields to suggested AI keys (e.g.,
    `English` -> `en`) for their confirmation or edits.
5.  **Save File:** Prompt for a filename and location to save the prompt
    template.
6.  **Confirmation:** Write the complete prompt file (frontmatter + boilerplate
    body, including HTML generation instructions) to the specified location and
    print a success message with an example command.

---

## 5. Implementation Tasks

1.  **[`generate` command]**
    - [ ] Create `src/commands/generate.ts`.
    - [ ] Implement a utility to parse files with YAML frontmatter.
    - [ ] Implement the main handler logic for `generate` as described in
          section 3.3.
    - [ ] Integrate an interactive checklist library (e.g., `inquirer`).
    - [ ] Adapt the existing batch processor to handle parallel requests with a
          single prompt template.
2.  **[`generate:init` command]**
    - [ ] Create `src/commands/generate-init.ts`.
    - [ ] Build the interactive wizard flow.
    - [ ] Implement the necessary AnkiConnect calls to fetch collection data.
    - [ ] Implement the logic to generate the complete prompt file content,
          including a boilerplate body that instructs the LLM to generate HTML
          where appropriate.
3.  **[Documentation]**
    - [ ] Update `README.md` with sections for the `generate` and
          `generate:init` commands.
    - [ ] Add `generate` to the "Example Workflows" section.
    - [ ] Document the prompt template structure, emphasizing that the LLM is
          responsible for generating final HTML.

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

- `-p, --prompt`: (Optional) Path to the prompt template file. Uses default from
  config if not specified.
- `-c, --count`: (Optional) The number of card examples to generate. Default:
  `3`.
- `-m, --model`: (Optional) The LLM model to use. Overrides the default set in
  `anki-llm config`.
- `--dry-run`: (Optional) Generate cards and display them in a formatted,
  human-readable list without starting the interactive selection or import
  process.
- `-b, --batch-size`: (Optional) Number of concurrent API requests. Default:
  `5`.
- `-r, --retries`: (Optional) Number of retries for failed requests. Default:
  `3`.

### 3.2. The Prompt Template

The prompt file is a text or markdown file containing YAML frontmatter for
configuration and a prompt body with instructions for the LLM.

#### 3.2.1. Frontmatter

The frontmatter is a YAML block enclosed by `---` at the top of the file.

- `deck`: (Required) The target Anki deck name.
- `noteType`: (Required) The name of the Anki note type (model) to be used.
- `fieldMap`: (Required) An object mapping the keys from the LLM's JSON output
  to the actual field names in the specified `noteType`. This provides a direct,
  one-to-one translation. Renamed from `map` for clarity.

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
deck: Japanese::Vocabulary
noteType: Japanese (recognition)
fieldMap:
  en: English
  jp: Japanese
  furigana: Furigana
  rom: Romaji
  context: Context
  note: Notes
---

You are an expert assistant who creates one excellent Anki flashcard for a
Japanese vocabulary word. The term to create a card for is: **{term}**

IMPORTANT: Your output must be a single, valid JSON object and nothing else. Do
not include any explanation, markdown formatting, or additional text. All field
values must be strings. For the `note` field, generate a single string
containing a well-formed HTML unordered list (`<ul>`).

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

Return only valid JSON matching this structure.
````

### 3.3. Execution Flow

1.  **Parse Arguments:** Read `<term>` and all options from the CLI.
2.  **Load & Parse Prompt File:** Read the file specified by `--prompt` (or use
    default from config). Use a new utility (`src/utils/parse-frontmatter.ts`)
    to parse the YAML frontmatter and separate it from the prompt body. Validate
    that required frontmatter fields are present.
3.  **Validate Deck & Note Type:** Check that the target deck and note type
    exist using AnkiConnect. If the deck doesn't exist, offer to create it or
    exit with an error. If the note type doesn't exist, exit with an error and
    helpful message.
4.  **Prepare API Calls:** Create an array of `--count` identical prompts, each
    with the `{term}` placeholder replaced using the existing template engine.
5.  **Execute in Parallel:** Build a lightweight generation-specific processor
    (don't reuse the heavy batch processing system). Make `--count` parallel API
    calls with retry logic (exponential backoff) and robust error handling.
    Implement robust JSON parsing: strip everything before the first `{` and
    after the last `}`, then `JSON.parse`.
6.  **Collect & Validate Results:** Parse and validate each response using Zod
    schema based on the `fieldMap` keys. Collect successful responses into "card
    candidates". If all API calls fail, exit with an error showing the failure
    reasons.
7.  **HTML Sanitization:** Strip `<script>` tags from all HTML field values to
    prevent XSS when syncing to AnkiWeb.
8.  **Duplicate Detection:** For each candidate card, check if it already exists
    in Anki by querying the first field (unique identifier). Mark duplicates and
    optionally filter them out or warn the user.
9.  **Interactive Selection:** If in `--dry-run` mode, display cards in a
    formatted, human-readable list and exit. Otherwise, present cards in an
    interactive checklist (using `inquirer` with paging for >10 items).
10. **Map and Import:**
    - Once the user confirms their selection, iterate through the chosen cards.
    - For each selected card, use the `fieldMap` from the frontmatter to
      transform the JSON object into the final Anki field structure (e.g.,
      `{"English": "...", "Japanese": "..."}`). All values are treated as
      strings and mapped directly.
    - Call the AnkiConnect `addNotes` action with the list of prepared notes.
11. **Report Outcome:** Print a summary to the user (e.g.,
    `✓ Successfully added 2 new notes to "Japanese::Vocabulary".`). Exit with
    code 0 for success, 1 for partial failure, >1 for hard error.

---

## 4. Onboarding Feature: `anki-llm generate-init`

A one-time scaffolding command will be created to generate a prompt template
based on the user's Anki collection. Note: Changed from `generate:init` to
`generate-init` for consistency with existing command naming patterns
(`process-deck`, `process-file`).

### 4.1. Goal

To create a prompt file for the user by interactively guiding them through
configuration, thus lowering the barrier to entry for the `generate` command.

### 4.2. Command Signature

```bash
anki-llm generate-init [output-file]
```

**Arguments:**

- `[output-file]`: (Optional) The path where the generated prompt file will be
  saved. Defaults to `generate-prompt.md` in the current directory.

### 4.3. Interactive Wizard Flow

1.  **Welcome:** Explain the command's purpose and what will be generated.
2.  **Select Deck:** Call AnkiConnect `deckNames` and present the list of decks
    for user selection.
3.  **Select Note Type:** Query the deck for all note types used (a deck may
    contain multiple note types). If multiple types exist, present a list for
    selection. If only one exists, auto-select it with confirmation.
4.  **Map Fields:** Fetch the field names for the selected note type using
    `modelFieldNames`. Present the user with a pre-filled mapping of Anki fields
    to suggested AI keys (e.g., `English` -> `en`, `Japanese` -> `jp`). For
    large field lists (>10), offer an auto-mapping option that matches identical
    field names automatically and only prompts for unmapped fields.
5.  **Save File:** Prompt for a filename and location to save the prompt
    template (default: `generate-prompt.md`).
6.  **Generate & Save:** Write the complete prompt file (frontmatter +
    boilerplate body) to the specified location. The boilerplate must include:
    - Clear "return JSON only" instructions
    - A one-shot example showing the exact JSON structure
    - Explicit HTML formatting guidance
7.  **Confirmation:** Print a success message with an example command to try:

    ```
    ✓ Prompt template saved to generate-prompt.md

    Try it out:
      anki-llm generate "example term" -p generate-prompt.md
    ```

---

## 5. Implementation Tasks

### 5.1. Core Utilities (Build First)

1.  **Frontmatter Parser**
    - [ ] Create `src/utils/parse-frontmatter.ts`
    - [ ] Implement YAML frontmatter parsing (split frontmatter from body)
    - [ ] Add Zod schema validation for required fields (`deck`, `noteType`,
          `fieldMap`)
    - [ ] Export types for use by command handlers
    - [ ] Design as reusable utility for potential future use by `process-*`
          commands

2.  **HTML Sanitization**
    - [ ] Create `src/utils/sanitize-html.ts`
    - [ ] Implement `<script>` tag stripping to prevent XSS
    - [ ] Consider using a lightweight library or simple regex approach
    - [ ] Add tests for edge cases

3.  **JSON Parsing with Fallback**
    - [ ] Create `src/utils/parse-llm-json.ts`
    - [ ] Implement robust JSON parsing: strip everything before first `{` and
          after last `}`
    - [ ] Handle common LLM output issues (markdown code blocks, extra text)
    - [ ] Return parsed object or throw descriptive error

### 5.2. Generation Infrastructure

4.  **Lightweight Generation Processor**
    - [ ] Create `src/generation/processor.ts`
    - [ ] Implement parallel API calls with concurrency control (using
          `p-limit`)
    - [ ] Add retry logic with exponential backoff (using `p-retry`)
    - [ ] Use standard chat completion API (no JSON mode for v1)
    - [ ] Return array of successful results + array of errors
    - [ ] **Do NOT reuse the existing batch processing system** - keep this
          simple and purpose-built

5.  **Card Validation & Deduplication**
    - [ ] Create `src/generation/validator.ts`
    - [ ] Validate card JSON against `fieldMap` using Zod
    - [ ] Implement duplicate detection using AnkiConnect `findNotes`
    - [ ] Mark duplicates in results without filtering (let user decide)

6.  **Interactive Selection UI**
    - [ ] Create `src/generation/selector.ts`
    - [ ] Integrate `inquirer` for interactive checklist
    - [ ] Add `@types/inquirer` dev dependency
    - [ ] Format card display (pretty-print key fields)
    - [ ] Support paging for >10 items

### 5.3. Commands

7.  **`generate` Command**
    - [ ] Create `src/commands/generate.ts`
    - [ ] Follow existing `Command<Args>` interface pattern from
          `commands/types.ts`
    - [ ] Implement all CLI options (see section 3.1)
    - [ ] Add shared options where applicable (model, batch-size, retries)
    - [ ] Implement full execution flow (section 3.3):
      - [ ] Load and parse prompt file with frontmatter
      - [ ] Validate deck and note type existence
      - [ ] Generate cards with retry logic
      - [ ] Sanitize HTML in results
      - [ ] Detect duplicates
      - [ ] Handle dry-run mode (formatted output)
      - [ ] Interactive selection
      - [ ] Map fields and add to Anki
      - [ ] Report results with proper exit codes
    - [ ] Register command in `src/cli.ts`

8.  **`generate-init` Command**
    - [ ] Create `src/commands/generate-init.ts`
    - [ ] Follow existing `Command<Args>` interface pattern
    - [ ] Implement wizard flow (section 4.3):
      - [ ] Welcome message
      - [ ] Deck selection (query AnkiConnect)
      - [ ] Note type selection (handle multiple types per deck)
      - [ ] Field mapping with auto-suggestions
      - [ ] Auto-mapping option for large field lists
      - [ ] Save prompt file with frontmatter + boilerplate
    - [ ] Generate comprehensive boilerplate with:
      - [ ] Clear "return JSON only" instructions
      - [ ] One-shot example
      - [ ] HTML formatting guidance
    - [ ] Print success message with example command
    - [ ] Register command in `src/cli.ts`

### 5.4. Configuration & Dependencies

9.  **Package Dependencies**
    - [ ] Add `inquirer` to dependencies (if not already present)
    - [ ] Add `@types/inquirer` to devDependencies
    - [ ] Verify `js-yaml` is available (already in dependencies)

10. **Config Schema Update**
    - [ ] Update `src/config.ts` to support optional `prompt` default
    - [ ] Update config commands to allow setting default prompt path
    - [ ] Document in config help text

### 5.5. Documentation

11. **README Updates**
    - [ ] Add `generate` command to Commands Reference section
    - [ ] Add `generate-init` command to Commands Reference section
    - [ ] Add example workflow to "Example Workflows" section
    - [ ] Show end-to-end example: init wizard → generate cards → import
    - [ ] Document prompt template structure with frontmatter
    - [ ] Emphasize that LLM generates final HTML (not markdown)
    - [ ] Include security note about HTML sanitization

12. **Example Prompts**
    - [ ] Create `examples/generate-japanese-vocab.md` with full example prompt
    - [ ] Include other language examples if applicable

### 5.6. Quality & Testing

13. **Error Handling Review**
    - [ ] Ensure all error cases have helpful messages
    - [ ] Verify exit codes (0 = success, 1 = partial, >1 = hard error)
    - [ ] Test with missing deck/note type
    - [ ] Test with malformed frontmatter
    - [ ] Test with invalid JSON responses
    - [ ] Test duplicate detection

14. **Edge Case Testing**
    - [ ] Test with count=1 and count=30
    - [ ] Test with non-existent prompt file
    - [ ] Test with empty/invalid frontmatter
    - [ ] Test when all API calls fail
    - [ ] Test when some API calls fail
    - [ ] Test dry-run mode
    - [ ] Test with different LLM models

---

## 6. Implementation Sequence Recommendation

Follow this order to minimize rework and enable incremental testing:

1. **Phase 1: Utilities** (tasks 1-3)
   - Build foundational utilities first
   - Test in isolation

2. **Phase 2: Generation Core** (tasks 4-6)
   - Build the generation engine
   - Test with mock prompts

3. **Phase 3: Generate Command** (task 7)
   - Wire up the full command
   - Test end-to-end

4. **Phase 4: Init Command** (task 8)
   - Build the wizard
   - Test with real Anki collections

5. **Phase 5: Polish** (tasks 9-14)
   - Update config, docs, examples
   - Comprehensive testing

---

## 7. Critical Design Notes

### 7.1. Keep It Simple

- **Do NOT reuse the batch processing system** (`processor.ts`,
  `core-processor.ts`, etc.)
- The existing batch system is designed for file-based workflows with
  incremental saves
- Build a lightweight, purpose-built processor for generation
- This avoids complexity and maintains clear separation of concerns

### 7.2. Security

- Always sanitize HTML before adding to Anki
- At minimum: strip `<script>` tags
- Consider stripping other potentially dangerous tags (`<iframe>`, `<object>`,
  etc.)

### 7.3. User Experience

- Provide helpful error messages for all failure modes
- Make duplicate detection transparent (show but don't auto-filter)
- Interactive-first design (non-interactive mode not needed for v1)

### 7.4. Consistency

- Follow existing command patterns (`Command<Args>` interface)
- Reuse shared options where applicable
- Match existing error handling and exit code conventions
- Use the same template engine (`{placeholder}` syntax)

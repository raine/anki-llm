## v2.0.2 (2026-04-06)

- Improved terminal output with colors and better-styled progress bars
- Card selector now shows full field content without truncation

## v2.0.0 (2026-04-06)

- Rewrote the entire tool in Rust - single self-contained binary, no Node.js
  runtime required

## v1.6.0 (2026-02-28)

- Added gemini-3-flash-preview and gemini-3.1-pro-preview model support
- Added gpt-4.1-mini, gpt-4.1-nano, gpt-5.1, and gpt-5.2 models
- Temperature parameter is now omitted for GPT-5 models (not supported by the
  API)

## v1.5.1 (2025-11-09)

- Arrays in LLM responses are now converted to HTML lists in `generate`
- Added progress spinners during prompt generation and card generation

## v1.5.0 (2025-11-09)

- Added optional quality check step in `generate` — the LLM reviews generated
  cards before presenting them for selection
- `generate-init` now shows which model was selected and improves the model
  selection flow
- Gemini is automatically used when only `GEMINI_API_KEY` is set, even if no
  OpenAI key is present

## v1.4.0 (2025-11-02)

- Added copy mode in `generate` — generated card fields can be copied to an
  existing note instead of creating a new one

## v1.3.0 (2025-11-02)

- LLM cost is now reported after `generate` and `generate-init` complete
- Card selector shows all fields of the selected card for easier review
- Fixed missing whitespace between HTML elements in card display
- Markdown is always converted to HTML in `generate` output
- Vim-style hotkeys enabled in interactive selection prompts

## v1.2.0 (2025-10-29)

- Added `generate` command — interactively generate multiple contextual
  flashcard examples for a term and add selected cards to your Anki deck
- Added `generate-init` wizard — uses an LLM to analyze your existing cards and
  produce a tailored prompt template file for the `generate` command
- `generate` now supports YAML output and can append to existing YAML/CSV files
  with schema validation
- Import command now auto-detects the key field from column names
- Added gpt-5 and gpt-5-mini model support
- Cards are validated concurrently during generation for faster feedback
- Model can be set in config and overridden per-command

## v1.1.0 (2025-10-25)

- Added `query` command for direct AnkiConnect access from the CLI or AI agents

## v1.0.1 (2025-10-25)

- Initial release
- `export` — export an Anki deck to CSV or YAML
- `import` — import a CSV or YAML file back into Anki
- `process-file` — file-based batch workflow: export, process with LLM, import
  (supports resume)
- `process-deck` — direct in-place batch processing of deck notes with an LLM
- OpenAI and Google Gemini model support
- Template-based prompt files with field substitution
- Concurrent processing with retry logic and cost tracking

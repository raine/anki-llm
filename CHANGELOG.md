## v2.0.4 (2026-04-10)

- Added prompt picker — prompts in the prompts directory are auto-discovered,
  and a picker appears when `--prompt` is omitted. Use Ctrl+P to switch prompts
  during a session. Last used prompt is remembered.
- Added model pricing display in the model picker
- Model can now be switched from the card selection, done, and error screens
  (Ctrl+O)
- Done screen now shows the final generated cards, allows copying card text, and
  deleting imported cards
- Added step history browsing on done and error screens — review what each
  processing step produced
- **Breaking:** `post_process` and `quality_check` prompt template keys have
  been replaced with a unified `processing` pipeline. See the README for the new
  configuration format.
- All LLM queries are now automatically logged to
  `~/.local/state/anki-llm/logs/`
- LLM errors are now logged to the session log file
- `generate-init` now outputs to the prompts directory by default
- Added gemini-3.1-flash-lite-preview model, removed gpt-5.4-pro
- Nerd Font checkbox glyphs are now configurable
- Fixed CJK characters bleeding into the sidebar
- Fixed cross-provider model overrides not applying in processing steps
- Fixed Ctrl+P not opening the picker when a prompt was already remembered

## v2.0.3 (2026-04-07)

- Added interactive TUI for the `generate` command — full-screen terminal
  interface with sidebar progress, card preview, and keyboard-driven workflow
- Added `post_process` support in prompt templates — delegate individual field
  generation to separate focused LLM calls for higher quality results
- **Breaking:** prompt template YAML keys switched from camelCase to snake_case
  (`noteType` → `note_type`, `fieldMap` → `field_map`, `qualityCheck` →
  `quality_check`). Old keys will produce a clear error prompting you to update.

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

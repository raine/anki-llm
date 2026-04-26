# Changelog

## v2.0.15 (2026-04-26)

- Add `doctor` command to diagnose configuration, API keys, AnkiConnect connectivity, and workspace setup.
- Add support for DeepSeek V4 models.
- Treat empty `DEEPSEEK_API_KEY` and `GEMINI_API_KEY` environment variables as unset, so they fall back to other configured providers.

## v2.0.14 (2026-04-26)

- Add `anki_connect_url` config key to customize the AnkiConnect endpoint (defaults to `http://localhost:8765`).
- `tts`: fall back to annotation-stripped text when furigana parsing fails, so audio still gets generated for notes with malformed ruby markup.

## v2.0.13 (2026-04-25)

- Add `note-type` command — pull Anki note type templates and CSS into local
  files, edit them in any editor, then push changes back to Anki. `note-type
status` shows a live diff of what has drifted.
- Add workspace support — a directory with a `prompts/` folder is now a
  workspace. Run `workspace init` to set one up. Prompts and settings are
  resolved from the current workspace automatically.
- Add `workspace init` and `workspace info` subcommands
- **Breaking:** `prompts_dir` config setting is replaced by `default_workspace`.
  Set it to your workspace directory and all commands resolve prompts, note
  types, and default model from there regardless of your working directory.
- `process-deck` now skips notes that already have content in the target field
  by default — use `--force` to re-process and overwrite existing values.

## v2.0.12 (2026-04-23)

- **Breaking:** `process-deck` and `process-file` prompts now require a YAML
  frontmatter block declaring the target field. The `--field`, `--json`, and
  `--require-result-tag` CLI flags are removed. Add a frontmatter block to each
  prompt file:

  ```
  ---
  output:
    field: Translation
    require_result_tag: true  # optional
  ---
  ```

- **Breaking:** JSON-merge output (`--json`) is no longer supported for
  `process-*`. Prompts now always write a single field.

- Add `--preview` flag to `process-deck` and `process-file` — process a small
  sample of cards with the LLM and show a diff-like summary of what would
  change, then prompt for confirmation before running the full batch.

## v2.0.11 (2026-04-23)

- Add `update` command — self-update to the latest release from GitHub
- Add `docs` command — show bundled documentation in the terminal

## v2.0.10 (2026-04-22)

- Add `z` key in the generate TUI to skip post-select processing — useful when
  you want to bypass quality checks or other post-processing steps configured in
  a prompt
- Fix large deck exports failing due to HTTP response buffer limit
  ([#3](https://github.com/raine/anki-llm/pull/3))

## v2.0.9 (2026-04-21)

- `tts-voices` browser now copies voice YAML to clipboard on Enter and stays
  open so you can keep exploring voices, instead of exiting immediately
- `generate` summary screen now lets you replay imported audio with `p`
- Furigana parser now accepts katakana loanwords with reading annotations (e.g.
  `スパイク[すぱいく]`), fixing TTS preparation for those cards

## v2.0.8 (2026-04-14)

### TTS (text-to-speech)

- Added `tts` command — synthesize audio for Anki card fields and upload to Anki
  as media
- Added support for multiple TTS providers: OpenAI, Azure Neural, Google Cloud,
  and Amazon Polly
- Added `tts-voices` subcommand — browse, filter, and preview voices from all
  providers in an interactive TUI
- TTS is now integrated into the `generate` pipeline — audio is synthesized
  automatically as a pipeline step before cards are added to Anki
- Added TTS preview hotkey in the generate selection screen — press P to preview
  audio before committing
- Added utterance IR with furigana parsing for better Japanese pronunciation

### `generate` improvements

- Post-select processing now shows per-card field diffs, making it easy to see
  what each processing step changed
- Added per-step cost reporting after post-select processing
- Added Summary step in the sidebar for run completion view
- Legacy (non-TUI) mode now correctly fails on import errors instead of silently
  exiting successfully

## v2.0.7 (2026-04-12)

- Added interactive batch TUI for `process-deck` — same live progress, row
  previews, and summary screen as `process-file`

## v2.0.6 (2026-04-11)

- Added interactive batch TUI for `process-file` — live progress with elapsed
  time, row previews, and a summary screen when done
- Added `--query` flag to `process-deck` and `export` to filter which notes are
  processed or exported
- Added snapshot/rollback system for `process-deck` — use `history` to view past
  runs and `rollback` to undo changes
- Added support for any OpenAI-compatible LLM provider — configure a custom
  endpoint and use any model

## v2.0.5 (2026-04-11)

### `generate`

- Added batch/multi-term input — paste multiple terms or press Tab to queue
  them, then process all at once into a single selection view
- Added single-card regeneration with feedback — press R on a card to regenerate
  it with custom guidance (e.g. "make the definition simpler")
- Added duplicate diffing — duplicate cards now show a field-by-field diff
  against the existing Anki note, and pressing F force-selects them when the new
  version is better
- Added $EDITOR integration — press E in the selection screen to edit a card's
  fields in your editor
- Added inline term input — press N in the selection screen to generate cards
  for a new term without leaving the view
- Added card removal — press X to remove unwanted cards from the selection
- Model picker now supports type-to-filter and Ctrl-N/Ctrl-P navigation
- Model changes in the selection screen are now deferred, showing which model
  generated each card

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

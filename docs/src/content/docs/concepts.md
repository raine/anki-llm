---
title: "Concepts"
description: "Understand Anki data, processing modes, prompt files, safety controls, rollback, and cost."
---

These concepts apply across the guides. Read this page once, then use each
task-specific guide for commands and options.

## Notes, fields, note types, and cards

Anki stores **notes**, not just cards.

- A **note** is one record of information, such as a word, its meaning, and an
  example sentence.
- A **field** is one named value on that note, such as `Expression`, `Meaning`,
  `Translation`, or `Audio`.
- A **note type**, also called a model in AnkiConnect, defines the fields a note
  has and the templates used to render cards.
- A **card** is a review item generated from a note by one of its note type's
  card templates. One note can produce multiple cards.
- A **deck** contains cards. A search can select notes through their cards, so
  one note may appear in more than one matching context.

Most `anki-llm` transformations operate on notes and fields. A processing prompt
reads fields through placeholders such as `{Japanese}` and declares the field or
fields it writes. Generation prompts map model output keys to fields on a target
note type. The field names must match the collection.

Use `anki-llm query modelNames`, then query fields with a real note type name,
for example `anki-llm query modelFieldNames '{"modelName":"Basic"}'`. An agent
can inspect the names before writing a prompt. See [Work with agents](/agents/)
for more query examples.

## File-based and direct processing

Both processing modes apply one prompt to many notes, but they place the review
boundary in different locations.

### File-based processing

The file workflow is:

1. Export notes from Anki to CSV or YAML.
2. Run `process-file` to produce a separate output file.
3. Review the output or inspect it with `git diff`.
4. Import the accepted file into Anki.

Use [Process a file](/process-file/) when you want a durable staging artifact,
need automatic resume for a large job, or want to process while Anki is closed.
Completed output rows are saved incrementally. Re-running the same command skips
complete rows unless you use `--force`.

### Direct processing

`process-deck` selects notes from a deck or Anki search and updates them through
AnkiConnect. It has fewer steps and automatically snapshots each run.

Use [Process a deck](/process-deck/) for focused updates where a preview and
rollback provide enough review. Direct processing does not resume an interrupted
run. Failed notes are written to an error log in the working directory.

:::caution[Existing field content]
Direct processing normally selects a note only when every declared output field
is empty. A partially populated output set is skipped. `--force` regenerates and
atomically overwrites the complete declared set, so preview the exact selection
before using it.
:::

## Prompt files are durable configuration

A prompt file is executable project configuration for an LLM task. It has two
parts:

1. YAML frontmatter declares machine-readable settings and output fields.
2. The body supplies instructions and `{FieldName}` placeholders.

For example:

```markdown
---
output:
  field: Translation
  require_result_tag: true
---

Translate {Japanese} into natural English.
Return only the translation inside <result></result> tags.
```

Keeping prompts as files makes them reviewable, repeatable, and versionable. A
small wording change can alter every generated value, so treat a prompt change
like a code change: inspect the diff, test a sample, and record the version used
for an important run.

Processing and generation use different frontmatter shapes. See
[Write prompts](/prompts/) for authoring guidance and
[Prompt reference](/prompt-reference/) for every supported key.

## Workspaces collect project files

A **workspace** is a directory recognized by an `anki-llm.yaml` file or a
`prompts/` directory. It gives commands a shared place to find:

- prompt files under `prompts/`
- note type HTML, CSS, and manifests under `note-types/`
- a default model and reasoning effort in `anki-llm.yaml`

The workspace in the current directory takes precedence. A configured
`default_workspace` supplies the fallback when you run commands elsewhere.
See [Use workspaces](/workspaces/) for setup and discovery rules.

## Preview, dry-run, and limit

These controls answer different safety questions.

### `--dry-run`

A dry run validates and displays an operation without applying its normal side
effects. For `process-file` and `process-deck`, it does not call the LLM. For
`generate`, it calls the LLM and displays generated cards but does not enter the
selection or import flow. Command-specific guides document the exact behavior.

Use it first to catch selection, placeholder, field, and configuration mistakes.

### `--preview`

Preview mode for `process-file` and `process-deck` sends a small sample to the
LLM, shows a diff-like summary, and asks for confirmation before the full run.
The default sample is three notes and `--preview-count` changes it.

Use it after a dry run to evaluate actual model output.

### `--limit`

`--limit N` caps the number of new rows or notes processed. It controls scope,
not whether model calls or writes occur. Combine it with dry-run or preview while
tuning a prompt, and use a small limit for the first applied run.

A safe progression is:

1. dry-run a small selection
2. preview a few real model responses
3. apply a small limited run
4. inspect the results in Anki or the output file
5. scale to the full selection

## Snapshots and rollback

Every `process-deck` run records the original note state under
`~/.local/state/anki-llm/snapshots/`. The command prints a run ID when it
finishes.

List available runs:

```sh
anki-llm history
```

Preview a rollback:

```sh
anki-llm rollback <run-id> --dry-run
```

Apply it after reviewing the preview:

```sh
anki-llm rollback <run-id>
```

Rollback checks whether fields changed after the run and skips conflicting notes.
`--force` overrides conflict detection and can overwrite later manual edits.

Note-type pushes use their own snapshots under
`~/.local/state/anki-llm/note-type-snapshots/` and also support a dry-run. An
exported file is the recovery and review boundary for `process-file`; keep the
original export until the import is verified.

:::important
A snapshot reduces recovery time, but it does not validate model output. Review
small samples before scaling and keep normal Anki backups.
:::

## Cost awareness

Each processed note usually produces one LLM request. Longer source fields,
longer instructions, multiple requested output fields, retries, and extra
processing steps all increase token use. Model prices can differ by orders of
magnitude.

`anki-llm` tracks tokens and prints estimated cost for known models. Unknown
models still work, but no estimate is shown. Provider prices can change, so use
the displayed estimate as guidance rather than an invoice.

To control cost:

- test prompt structure with `--dry-run`, which makes no model request for batch
  processing
- use `--preview` or `--limit` on a representative sample
- inspect the sample's token and cost totals, then extrapolate
- choose a smaller model for mechanical transformations
- reserve capable reasoning models for prompts that benefit from them
- remember that generation processing steps add model calls for each candidate
  that reaches the step
- avoid `--force` unless intentional, because it can repeat completed work

Cost control and quality control use the same discipline: start with a small,
representative sample and inspect it before scaling.

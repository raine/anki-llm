---
title: "Process a file"
description: "Export notes, process them with an LLM, and review the results before importing them."
---

Use the file workflow when you want a reviewable artifact between Anki and the
LLM. It is a good fit for large jobs because progress is saved incrementally and
the same command resumes an interrupted run.

The workflow has three stages:

1. Export notes from Anki to CSV or YAML.
2. Process the exported rows into a separate output file.
3. Review the output and import it into Anki.

:::tip
YAML preserves multiline fields cleanly and produces readable diffs. CSV is
convenient for spreadsheets. Input and output files may use `.csv`, `.yaml`, or
`.yml` extensions.
:::

## 1. Export the notes

Anki must be open with AnkiConnect installed while exporting:

```sh
anki-llm export "Japanese Core 1k" --output notes.yaml
```

When you select a deck and omit `--output`, anki-llm derives a YAML filename from
the deck name, such as `Japanese Core 1k` to `japanese-core-1k.yaml`. A
`--query` export requires an explicit output path:

```sh
anki-llm export --query 'deck:Japanese -field:Audio' --output missing-audio.yaml
```

You can export a deck or select notes with an Anki search query. Each exported
row includes a note ID, which `process-file` uses to match input rows with saved
output and `import` uses to update the original note.

See [`export` in the command reference](/command-reference/#command-export) for deck,
query, note type, and output options.

## 2. Write a processing prompt

A batch prompt declares its destination field in YAML frontmatter and refers to
source fields with placeholders:

```markdown
---
output:
  field: Translation
  require_result_tag: true
---

Translate this Japanese sentence into natural English: {Japanese}

Return the final translation inside <result></result> tags.
```

See [Write batch prompts](/prompts/) for single-field and multi-field prompts,
validation rules, result tags, and staging fields.

## 3. Preview the operation

Start with a dry run. It prints the prompt template, a sample row, and the filled
sample prompt without calling the model or writing output:

```sh
anki-llm process-file notes.yaml \
  --output notes-processed.yaml \
  --prompt prompt.md \
  --dry-run
```

Then process a small sample with the model:

```sh
anki-llm process-file notes.yaml \
  --output notes-processed.yaml \
  --prompt prompt.md \
  --preview \
  --preview-count 3
```

Preview mode shows a field-by-field summary and asks whether to continue. If you
cancel, it makes no changes. Use `--limit 20` when you want to produce a small
output file, inspect it in full, and estimate quality and cost before scaling up.

:::caution
A dry run validates template expansion but cannot judge model output. Use a
model-backed preview or a limited run before processing a large collection.
:::

## 4. Process the file

Run the full job after the sample looks right:

```sh
anki-llm process-file notes.yaml \
  --output notes-processed.yaml \
  --prompt prompt.md \
  --model gemini-2.5-flash \
  --batch-size 10
```

Each row becomes one model request. Requests run concurrently according to the
batch size, and transient failures use the configured retry count. The terminal
UI shows progress, token use, and estimated cost. When output is piped, the
command uses plain progress output instead.

For the complete option list, including runtime, model, retry, and diagnostic
settings, see [`process-file` in the command reference](/command-reference/#command-process-file).

### Resume an interrupted run

`process-file` writes successful rows to the output file incrementally. Rerun
the same command with the same output path to resume:

```sh
anki-llm process-file notes.yaml \
  --output notes-processed.yaml \
  --prompt prompt.md
```

Rows are matched by `noteId`, `id`, or `Id`. Input IDs must be present and
unique. A corrupt output file or duplicate output ID stops the run rather than
silently discarding saved progress.

Completion depends on the prompt shape:

- For `output.field`, a row is complete when the output contains that field.
- For `output.fields`, a row is complete only when every declared field has a
  populated value.
- Rows carrying a processing error, and partially populated multi-field rows,
  remain eligible for retry.

Use `--force` only when you intend to ignore saved output and process every
input row again.

### Understand multi-field writes

A multi-field prompt makes one request per row and expects one structured JSON
object containing every declared field. Validation happens before the row is
saved. If a response is missing a field, contains an empty value, uses a
non-string value, or includes an undeclared field, the whole row fails and none
of its declared fields are applied.

This all-or-nothing behavior prevents related fields such as `Reading`,
`Explanation`, and `KanjiBreakdown` from getting out of sync. Source and
undeclared fields are preserved in the output.

### Capture raw model traffic

When a prompt behaves unexpectedly, append raw prompts, responses, and errors to
a log file:

```sh
anki-llm process-file notes.yaml \
  --output notes-processed.yaml \
  --prompt prompt.md \
  --log process.log
```

The command reference documents the additional terminal logging mode. Logs can
contain complete card fields and model output, so store and share them with the
same care as your collection.

## 5. Review and import

Inspect the output before updating Anki. For YAML tracked in Git, a direct diff
is useful:

```sh
git diff --no-index notes.yaml notes-processed.yaml
```

When the changes look right, open Anki and import them:

```sh
anki-llm import notes-processed.yaml --deck "Japanese Core 1k"
```

The importer identifies existing notes with `--key-field` when supplied. Without
it, the importer uses `noteId` when that column exists, then falls back to the
note type's first field. Matching rows update existing notes, while unmatched
rows create notes in the destination deck. Blank, duplicate, or ambiguous keys
stop the import instead of choosing a note nondeterministically.

Destination note fields that are absent from the file are preserved, and file
fields that do not exist on the destination note type are ignored. See
[`import` in the command reference](/command-reference/#command-import) for its
complete options.

## When Anki can be closed

Anki and AnkiConnect are required for `export` and `import`. The middle
`process-file` stage reads and writes local files only, so Anki can remain closed
for the entire LLM run. This makes file mode suitable for long or remote
processing sessions.

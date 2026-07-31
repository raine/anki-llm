---
title: "Process a deck"
description: "Run an LLM over selected notes and update them directly in Anki."
---

`process-deck` sends selected notes to an LLM and writes validated results back
to Anki without an intermediate data file. Use it for focused jobs where direct
updates and automatic rollback snapshots are more convenient than an
export-review-import cycle.

:::caution
Anki Desktop must stay open with AnkiConnect installed for the entire run.
`process-deck` updates the collection as successful batches finish and does not
support resume. Use [the file workflow](/process-file/) for a long run where an
interruption is likely.
:::

## Prepare the prompt

Batch prompts declare one or more Anki fields to write. This example fills an
empty `Hint` field:

```markdown
---
output:
  field: Hint
---

Write a short hint for this card without giving away the answer.

Question: {Front}
Answer: {Back}
```

See [Write batch prompts](/prompts/) for placeholders, structured multi-field
output, validation, and result tags.

## Select notes

Pass either a deck name or an Anki search query:

```sh
anki-llm process-deck "Japanese Core 1k" --prompt hint.md
```

```sh
anki-llm process-deck \
  --query 'deck:Japanese prop:lapses>5' \
  --prompt explanation.md
```

Queries let you target useful subsets, for example `tag:leech` or `rated:7:1`.
If the selection contains more than one note type, filter it with the note-type
option so every selected note has the same fields.

See [`process-deck` in the command reference](/command-reference/#command-process-deck)
for the exact selection syntax and complete option list.

## Preview safely

First inspect template expansion without making an API request or changing
Anki:

```sh
anki-llm process-deck "Japanese Core 1k" \
  --prompt hint.md \
  --limit 10 \
  --dry-run
```

Then ask the model to process a small sample and review the proposed changes:

```sh
anki-llm process-deck "Japanese Core 1k" \
  --prompt hint.md \
  --preview \
  --preview-count 3
```

Preview mode displays each changed field and asks for confirmation before the
full run. Cancelling the preview leaves Anki unchanged. A small `--limit` run is
also useful for checking real cards, cost, and formatting before broadening the
selection.

## Protect existing fields

By default, a note is processed only when every declared output field is empty.
If any target already contains content, the note is skipped.

This rule is especially important for multi-field prompts. Suppose a prompt
declares:

```yaml
output:
  fields:
    - Reading
    - Explanation
    - KanjiBreakdown
```

A note with content in any one of those fields is skipped. Passing `--force`
regenerates and atomically overwrites the complete declared set:

```sh
anki-llm process-deck "Japanese Core 1k" \
  --prompt enrich.md \
  --preview \
  --force
```

The model response must validate for every field before any field on that note
is updated. A failed or invalid response leaves the complete target set
unchanged.

:::danger
`--force` authorizes replacement of existing target content. Always combine it
with a narrow query, a limit, or `--preview` until you have verified the prompt.
:::

## Run the update

After the preview looks right, run the selected scope:

```sh
anki-llm process-deck \
  --query 'deck:Japanese tag:needs-hint' \
  --prompt hint.md \
  --model gemini-2.5-flash
```

Successful batches are written directly to Anki. The UI reports progress, token
use, estimated cost, and the final snapshot run ID.

### Inspect failures

Failed notes are written as JSON Lines in the working directory. The filename is
based on the deck or query slug, such as:

```text
japanese-core-1k-errors.jsonl
```

Each record includes the note data and error, which makes it possible to diagnose
placeholder, API, and response-validation failures. Raw request logging is also
available when you need to inspect complete model prompts and responses. See the
[command reference](/command-reference/#command-process-deck) for logging options.

Error logs and raw LLM logs may contain private card content. Keep them out of
public repositories.

## Review history and roll back

Every run that updates at least one note saves the original field values in a
snapshot under:

```text
~/.local/state/anki-llm/snapshots/
```

List available runs:

```sh
anki-llm history
```

The table shows each run ID, source, model, note count, and rollback status. Use
the run ID to preview a restore:

```sh
anki-llm rollback 20260411T153000_123Z --dry-run
```

Apply the rollback after reviewing it:

```sh
anki-llm rollback 20260411T153000_123Z
```

Rollback checks whether fields changed after the processing run. If a note was
edited later in Anki, that note is reported as a conflict and skipped, which
protects the newer edit. A forced rollback overrides conflict detection and
restores the snapshot values, so reserve it for cases where the snapshot is the
intended source of truth.

See [`history`](/command-reference/#command-history) and
[`rollback`](/command-reference/#command-rollback) for their complete command options.

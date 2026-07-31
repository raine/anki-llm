---
title: "Write prompts"
description: "Author reliable prompt files for process-file and process-deck."
---

Batch prompt files tell `process-file` and `process-deck` what to send to the
model and which Anki fields may receive the result. They are plain text or
Markdown files with YAML frontmatter followed by a prompt body.

:::note
Generation prompts use a different frontmatter schema with `deck`, `note_type`,
and `field_map`. See [Generate cards](/generate/) for that workflow.
:::

## Start with one output field

Declare the destination Anki field under `output.field`:

```markdown
---
output:
  field: Translation
---

Translate this sentence into natural English.

Japanese: {Japanese}

Return only the translation.
```

For each note or row, `anki-llm` replaces `{Japanese}` with that field's raw
value and writes the model's plain-text response to `Translation`.

Use raw Anki field names inside placeholders. Matching is case-insensitive, so
`{japanese}` can read a field named `Japanese`. An unknown placeholder fails the
individual row rather than sending an incomplete prompt.

## Isolate a result from explanatory output

Sometimes a model performs better when it can explain its decision before
returning the value to save. Enable result-tag extraction for a single output
field:

```markdown
---
output:
  field: Translation
  require_result_tag: true
---

Translate: {Japanese}

Briefly analyze important ambiguity, then put only the final translation in
<result></result> tags.
```

Only the content inside the last complete `<result>...</result>` pair is
written. A response without the tags fails validation and follows normal retry
behavior.

:::caution
`require_result_tag` is valid only with `output.field`. Do not use it with
structured multi-field output.
:::

## Produce related fields together

Use `output.fields` when one model call should create two or more related
values:

```markdown
---
output:
  fields:
    - Reading
    - Explanation
    - KanjiBreakdown
---

Japanese sentence: {Kanji}
English translation: {Front}
Grammar point: {BunproGrammar}
Meaning: {BunproMeaning}

Return Reading, Explanation, and KanjiBreakdown for this card.
```

The model is instructed to return one JSON object with exactly the declared
keys:

```json
{
  "Reading": "誰[だれ]もがお 寿司[すし]が 好[す]きだとは 限[かぎ]らない。",
  "Explanation": "とは限らない means that something is not necessarily true.",
  "KanjiBreakdown": "<div class=\"kanji-breakdown\">...</div>"
}
```

Every declared field must be present as a non-empty string, and undeclared keys
are rejected. The complete response is validated before any target field is
applied. If validation fails, none of the fields for that row or note change.

Multi-field output is useful when values must agree with each other. It also
means one request, one retry sequence, and one token and cost record for the
complete field set.

## Use staging fields

A file workflow can carry source-only fields that help the model but do not
exist on the destination Anki note type. For example, a preprocessing script
could add `DictionaryDefinition` to exported YAML:

```markdown
---
output:
  field: Hint
---

Word: {Expression}
Dictionary definition: {DictionaryDefinition}

Write a concise learner hint without repeating the word.
```

`process-file` preserves the staging field in its output. During import,
`anki-llm` ignores fields that are absent from the destination note type, while
updating valid destination fields.

:::tip
Keep source facts in placeholders and output requirements in explicit
instructions. This makes a prompt easier to test and reduces accidental copying
of labels or commentary into the target field.
:::

## Validate a prompt in stages

Use this sequence when authoring or changing a prompt:

1. Run `--dry-run` to inspect one filled prompt without an API call.
2. Run `--preview` to inspect model output for a few notes.
3. Run with `--limit 20` and review the resulting file or Anki notes.
4. Scale up only after field names, formatting, and cost look right.
5. Add a raw request log when response validation still fails.

For file-backed work, see [Process a file](/process-file/). For direct updates,
see [Process a deck](/process-deck/).

## Prompt authoring checklist

Before a large run, confirm that:

- the file starts with a valid YAML block enclosed by `---` lines;
- exactly one of `output.field` or `output.fields` is declared;
- `output.fields` contains at least two unique, non-empty field names;
- every placeholder exists on every selected row or note;
- the requested response is concise enough for the target field;
- HTML, furigana, or other formatting has a concrete example;
- result tags are requested in the body when they are required in frontmatter;
- a multi-field prompt asks for all declared fields and no extras.

The [prompt reference](/prompt-reference/) documents the full schema and response
rules.

## Runnable examples

Use the repository examples as starting points:

- [Key Vocabulary batch prompt](https://github.com/raine/anki-llm/blob/main/examples/key_vocabulary.md)
- [Japanese contextual generation prompt](https://github.com/raine/anki-llm/blob/main/examples/japanese_contextual.md)
- [Vocabulary generation prompt](https://github.com/raine/anki-llm/blob/main/examples/generate_vocab.md)

The [translation recipe](/recipes/translations/) and
[Key Vocabulary recipe](/recipes/key-vocabulary/) show complete batch workflows.
